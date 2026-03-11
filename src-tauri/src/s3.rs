use std::path::Path;
use std::time::Instant;

use aws_config::BehaviorVersion;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

use aws_sdk_s3::error::ProvideErrorMetadata;

use crate::error::AppError;
use crate::throttle::ThrottleState;

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

/// Minimum S3 multipart part size (5 MiB — S3 hard minimum for all but the last part).
const MULTIPART_PART_SIZE: usize = 5 * 1024 * 1024;


/// Default chunk size used when streaming a throttled download.
const DOWNLOAD_CHUNK_SIZE: usize = 256 * 1024; // 256 KiB

#[derive(Debug, serde::Serialize)]
pub struct S3Object {
    pub key: String,
    pub size: i64,
}

pub struct S3Client {
    client: Client,
    bucket: String,
    throttle: ThrottleState,
    /// Multipart upload part size in bytes. Files at or above this size use multipart upload.
    /// Each part is uploaded as a separate HTTP request; larger parts = fewer requests = faster
    /// for big files, but each part is held in RAM temporarily (~2× this value peak).
    part_size_bytes: usize,
}

impl S3Client {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        endpoint: &str,
        region: Option<&str>,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        extra_env: Option<&str>,
        throttle: ThrottleState,
        part_size_bytes: usize,
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
        // WhenRequired only sends a checksum when the operation mandates it.
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
            part_size_bytes: part_size_bytes.max(MULTIPART_PART_SIZE),
        })
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

    /// Upload a file to S3.
    ///
    /// Files at or above `LARGE_FILE_THRESHOLD` (100 MiB) always use multipart
    /// upload.  When a rate limit is configured, multipart is used for all
    /// files so that pacing sleeps can be inserted between parts.
    pub async fn upload_object(&self, key: &str, file_path: &Path) -> Result<(), AppError> {
        self.upload_object_impl(key, file_path, None).await
    }

    /// Like `upload_object` but calls `on_progress(bytes_done, bytes_total)` after
    /// each uploaded part so callers can emit progress events.
    pub async fn upload_object_with_progress(
        &self,
        key: &str,
        file_path: &Path,
        on_progress: impl Fn(u64, u64) + Send + Sync + 'static,
    ) -> Result<(), AppError> {
        self.upload_object_impl(key, file_path, Some(Box::new(on_progress))).await
    }

    async fn upload_object_impl(
        &self,
        key: &str,
        file_path: &Path,
        on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<(), AppError> {
        let upload_bps = self.throttle.get_upload_bps();
        let file_size = std::fs::metadata(file_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);

        let use_multipart = upload_bps > 0 || file_size >= self.part_size_bytes;

        if use_multipart {
            self.upload_object_multipart_throttled(key, file_path, self.part_size_bytes, upload_bps, on_progress)
                .await?;
        } else {
            let body = ByteStream::from_path(file_path)
                .await
                .map_err(|e| AppError::S3(format!("Failed to read file for upload: {}", e)))?;

            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .body(body)
                .send()
                .await
                .map_err(|e| s3_err("PutObject", e))?;

            if let Some(cb) = &on_progress {
                cb(file_size as u64, file_size as u64);
            }
        }

        Ok(())
    }

    /// Download an S3 object to a local file, applying the download rate limit
    /// when one is configured.
    pub async fn download_object(&self, key: &str, file_path: &Path) -> Result<(), AppError> {
        self.download_object_impl(key, file_path, None).await
    }

    /// Like `download_object` but calls `on_progress(bytes_done, bytes_total)` after
    /// each downloaded chunk so callers can emit progress events.
    /// `bytes_total` is taken from the `Content-Length` response header.
    pub async fn download_object_with_progress(
        &self,
        key: &str,
        file_path: &Path,
        on_progress: impl Fn(u64, u64) + Send + Sync + 'static,
    ) -> Result<(), AppError> {
        self.download_object_impl(key, file_path, Some(Box::new(on_progress))).await
    }

    async fn download_object_impl(
        &self,
        key: &str,
        file_path: &Path,
        on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<(), AppError> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| s3_err("GetObject", e))?;

        let content_length = resp.content_length().unwrap_or(0) as u64;
        let download_bps = self.throttle.get_download_bps();

        // Always stream in chunks so we can report progress regardless of throttle setting.
        use std::io::Write;
        let mut file = std::fs::File::create(file_path).map_err(AppError::Io)?;
        let mut body = resp.body;
        let mut total_written: u64 = 0;
        let mut batch_bytes: u64 = 0;
        let mut batch_start = Instant::now();
        let mut chunk_buf: Vec<u8> = Vec::with_capacity(DOWNLOAD_CHUNK_SIZE);

        while let Some(chunk) = body.next().await {
            let bytes = chunk
                .map_err(|e| AppError::S3(format!("Failed to read S3 response body: {}", e)))?;
            chunk_buf.extend_from_slice(&bytes);

            // Flush when we've accumulated a full chunk
            if chunk_buf.len() >= DOWNLOAD_CHUNK_SIZE {
                let n = chunk_buf.len() as u64;
                file.write_all(&chunk_buf).map_err(AppError::Io)?;
                chunk_buf.clear();
                total_written += n;
                batch_bytes += n;

                if let Some(cb) = &on_progress {
                    cb(total_written, content_length);
                }

                if download_bps > 0 {
                    crate::throttle::enforce_rate(batch_bytes, batch_start, download_bps).await;
                    batch_bytes = 0;
                    batch_start = Instant::now();
                }
            }
        }

        // Flush remainder
        if !chunk_buf.is_empty() {
            let n = chunk_buf.len() as u64;
            file.write_all(&chunk_buf).map_err(AppError::Io)?;
            total_written += n;
            if let Some(cb) = &on_progress {
                cb(total_written, content_length);
            }
        }

        Ok(())
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
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket);

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


    async fn upload_object_multipart_throttled(
        &self,
        key: &str,
        file_path: &Path,
        part_size: usize,
        _upload_bps: u64,
        on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<(), AppError> {
        use std::io::Read;
        let total_size = std::fs::metadata(file_path)?.len() as usize;

        // S3 requires at least MULTIPART_PART_SIZE per part (except the last).
        // For tiny files fall through to a regular PutObject.
        if total_size < MULTIPART_PART_SIZE {
            let body = ByteStream::from_path(file_path)
                .await
                .map_err(|e| AppError::S3(format!("Failed to read file for upload: {}", e)))?;
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .body(body)
                .send()
                .await
                .map_err(|e| s3_err("PutObject", e))?;
            return Ok(());
        }

        let effective_part_size = part_size.max(MULTIPART_PART_SIZE);

        let create_resp = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| s3_err("CreateMultipartUpload", e))?;

        let upload_id = create_resp
            .upload_id()
            .ok_or_else(|| AppError::S3("No upload_id returned".into()))?
            .to_string();

        let mut completed_parts = Vec::new();
        let mut part_number = 1i32;
        let mut offset = 0usize;
        let mut bytes_uploaded: u64 = 0;

        // Open the file once and read sequentially — avoids loading the entire
        // encrypted blob into memory before uploading.
        let mut file = std::fs::File::open(file_path)?;

        let result: Result<(), AppError> = async {
            while offset < total_size {
                let end = std::cmp::min(offset + effective_part_size, total_size);
                let part_bytes = (end - offset) as u64;

                let mut part_data = vec![0u8; end - offset];
                file.read_exact(&mut part_data)?;

                let part_start = Instant::now();

                let resp = self
                    .client
                    .upload_part()
                    .bucket(&self.bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .part_number(part_number)
                    .body(ByteStream::from(part_data))
                    .send()
                    .await
                    .map_err(|e| {
                        let detail = match (e.code(), e.message()) {
                            (Some(c), Some(m)) => format!("{}: {}", c, m),
                            (Some(c), None) => c.to_string(),
                            (None, Some(m)) => m.to_string(),
                            (None, None) => format!("{:?}", e),
                        };
                        AppError::S3(format!("UploadPart {} failed: {}", part_number, detail))
                    })?;

                let etag = resp.e_tag().unwrap_or_default().to_string();
                completed_parts.push(
                    aws_sdk_s3::types::CompletedPart::builder()
                        .part_number(part_number)
                        .e_tag(etag)
                        .build(),
                );

                bytes_uploaded += part_bytes;
                if let Some(cb) = &on_progress {
                    cb(bytes_uploaded, total_size as u64);
                }

                // Re-read bps in case it was updated during the transfer
                let current_bps = self.throttle.get_upload_bps();
                crate::throttle::enforce_rate(part_bytes, part_start, current_bps).await;

                offset = end;
                part_number += 1;
            }
            Ok(())
        }
        .await;

        match result {
            Ok(()) => {
                let completed = aws_sdk_s3::types::CompletedMultipartUpload::builder()
                    .set_parts(Some(completed_parts))
                    .build();

                self.client
                    .complete_multipart_upload()
                    .bucket(&self.bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .multipart_upload(completed)
                    .send()
                    .await
                    .map_err(|e| s3_err("CompleteMultipartUpload", e))?;
                Ok(())
            }
            Err(e) => {
                // Abort on failure
                let _ = self
                    .client
                    .abort_multipart_upload()
                    .bucket(&self.bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .send()
                    .await;
                Err(e)
            }
        }
    }
}
