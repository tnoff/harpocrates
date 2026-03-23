use std::time::Instant;

use aws_config::BehaviorVersion;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

use crate::error::AppError;
use crate::throttle::ThrottleState;

/// Streaming buffer size used when downloading a chunk with throttle applied.
const DOWNLOAD_STREAM_CHUNK: u64 = 256 * 1024; // 256 KiB

/// Convert an S3 SDK error into an AppError, extracting the AWS error code
/// and message instead of letting the SDK's Display collapse them to "service error".
fn s3_err<E>(op: &str, e: aws_sdk_s3::error::SdkError<E>) -> AppError
where
    E: ProvideErrorMetadata + std::fmt::Debug,
{
    let detail = match (e.code(), e.message()) {
        (Some(code), Some(msg)) => format!("{}: {}", code, msg),
        (Some(code), None) => code.to_string(),
        (None, Some(msg)) => msg.to_string(),
        (None, None) => format!("{:?}", e),
    };
    AppError::S3(format!("{} failed: {}", op, detail))
}

#[derive(Debug, serde::Serialize)]
pub struct S3Object {
    pub key: String,
    pub size: i64,
}

/// S3 client wrapper.  Cloneable so it can be shared across upload worker tasks.
#[derive(Clone)]
pub struct S3Client {
    client: Client,
    bucket: String,
    throttle: ThrottleState,
}

impl S3Client {
    pub async fn new(
        endpoint: &str,
        region: Option<&str>,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        extra_env: Option<&str>,
        throttle: ThrottleState,
    ) -> Result<Self, AppError> {
        // Apply extra environment variables before loading SDK config.
        // Format: comma-separated KEY=value pairs (e.g. "KEY=val,KEY2=val2").
        if let Some(env_str) = extra_env {
            for pair in env_str.split(',') {
                if let Some((k, v)) = pair.trim().split_once('=') {
                    std::env::set_var(k.trim(), v.trim());
                }
            }
        }

        let credentials = Credentials::new(access_key, secret_key, None, None, "harpocrates");
        let region = Region::new(region.unwrap_or("us-east-1").to_string());

        let config = aws_config::defaults(BehaviorVersion::latest())
            .credentials_provider(credentials)
            .region(region)
            .endpoint_url(endpoint)
            .load()
            .await;

        // Disable checksum trailer mode: many S3-compatible providers (B2, R2, MinIO, etc.)
        // reject chunked transfer encoding that the SDK uses when checksums are always computed.
        let s3_config = aws_sdk_s3::config::Builder::from(&config)
            .force_path_style(true)
            .request_checksum_calculation(
                aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired,
            )
            .response_checksum_validation(
                aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired,
            )
            .build();

        let client = Client::from_conf(s3_config);

        Ok(Self {
            client,
            bucket: bucket.to_string(),
            throttle,
        })
    }

    /// Create a non-functional client for use in tests that exercise code paths
    /// which return before any S3 operation (e.g. `Skipped`, `Deduped`).
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        let creds = Credentials::new("test", "test", None, None, "test");
        let s3_config = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .credentials_provider(creds)
            .region(Region::new("us-east-1"))
            .endpoint_url("http://localhost:1")
            .build();
        Self {
            client: Client::from_conf(s3_config),
            bucket: "test-bucket".to_string(),
            throttle: ThrottleState::new(),
        }
    }

    pub async fn head_bucket(&self) -> Result<(), AppError> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|e| s3_err("HeadBucket", e))?;
        Ok(())
    }

    /// Upload an in-memory encrypted chunk to S3 using a single PutObject.
    ///
    /// The upload throttle is applied after the request completes, pacing
    /// the overall throughput when many chunks are uploaded sequentially or
    /// in a bounded worker pool.
    pub async fn upload_chunk(&self, key: &str, data: Vec<u8>) -> Result<(), AppError> {
        let upload_bps = self.throttle.get_upload_bps();
        let byte_count = data.len() as u64;
        let start = Instant::now();

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .send()
            .await
            .map_err(|e| s3_err("PutObject", e))?;

        crate::throttle::enforce_rate(byte_count, start, upload_bps).await;
        Ok(())
    }

    /// Download a chunk from S3 and return its raw bytes.
    ///
    /// The download throttle is applied progressively as the response body
    /// streams in (paced every `DOWNLOAD_STREAM_CHUNK` bytes).
    pub async fn download_chunk(&self, key: &str) -> Result<Vec<u8>, AppError> {
        let download_bps = self.throttle.get_download_bps();

        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| s3_err("GetObject", e))?;

        let content_length = resp.content_length().unwrap_or(0) as usize;
        let mut buf = Vec::with_capacity(content_length.max(1));
        let mut body = resp.body;
        let mut batch_bytes: u64 = 0;
        let mut batch_start = Instant::now();

        while let Some(chunk) = body.next().await {
            let bytes = chunk
                .map_err(|e| AppError::S3(format!("Failed to read S3 response body: {}", e)))?;
            batch_bytes += bytes.len() as u64;
            buf.extend_from_slice(&bytes);

            if download_bps > 0 && batch_bytes >= DOWNLOAD_STREAM_CHUNK {
                crate::throttle::enforce_rate(batch_bytes, batch_start, download_bps).await;
                batch_bytes = 0;
                batch_start = Instant::now();
            }
        }

        // Pace any remaining bytes not yet throttled
        if download_bps > 0 && batch_bytes > 0 {
            crate::throttle::enforce_rate(batch_bytes, batch_start, download_bps).await;
        }

        Ok(buf)
    }

    pub async fn delete_object(&self, key: &str) -> Result<(), AppError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| s3_err("DeleteObject", e))?;
        Ok(())
    }

    pub async fn copy_object(&self, source_key: &str, dest_key: &str) -> Result<(), AppError> {
        let copy_source = format!("{}/{}", self.bucket, source_key);
        self.client
            .copy_object()
            .bucket(&self.bucket)
            .copy_source(&copy_source)
            .key(dest_key)
            .send()
            .await
            .map_err(|e| s3_err("CopyObject", e))?;
        Ok(())
    }

    pub async fn list_objects(&self, prefix: Option<&str>) -> Result<Vec<S3Object>, AppError> {
        let mut objects = Vec::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let mut req = self.client.list_objects_v2().bucket(&self.bucket);

            if let Some(p) = prefix {
                req = req.prefix(p);
            }

            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| s3_err("ListObjectsV2", e))?;

            for obj in resp.contents() {
                if let Some(key) = obj.key() {
                    objects.push(S3Object {
                        key: key.to_string(),
                        size: obj.size.unwrap_or(0),
                    });
                }
            }

            if resp.is_truncated() == Some(true) {
                continuation_token = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }

        Ok(objects)
    }
}
