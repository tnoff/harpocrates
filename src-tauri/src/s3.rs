use std::path::Path;
use std::time::Instant;

use aws_config::BehaviorVersion;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

use crate::error::AppError;
use crate::throttle::ThrottleState;

/// Minimum S3 multipart part size (5 MiB — S3 hard minimum for all but the last part).
const MULTIPART_PART_SIZE: usize = 5 * 1024 * 1024;

/// Files at or above this size automatically use multipart upload.
const LARGE_FILE_THRESHOLD: usize = 100 * 1024 * 1024; // 100 MiB

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
        // Apply extra environment variables if provided
        if let Some(env_json) = extra_env {
            if let Ok(vars) = serde_json::from_str::<std::collections::HashMap<String, String>>(env_json) {
                for (k, v) in vars {
                    std::env::set_var(&k, &v);
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

        let s3_config = aws_sdk_s3::config::Builder::from(&config)
            .force_path_style(true)
            .build();

        let client = Client::from_conf(s3_config);

        Ok(Self {
            client,
            bucket: bucket.to_string(),
            throttle,
        })
    }

    pub async fn head_bucket(&self) -> Result<(), AppError> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|e| AppError::S3(format!("HeadBucket failed: {}", e)))?;
        Ok(())
    }

    /// Upload a file to S3.
    ///
    /// Files at or above `LARGE_FILE_THRESHOLD` (100 MiB) always use multipart
    /// upload.  When a rate limit is configured, multipart is used for all
    /// files so that pacing sleeps can be inserted between parts.
    pub async fn upload_object(&self, key: &str, file_path: &Path) -> Result<(), AppError> {
        let upload_bps = self.throttle.get_upload_bps();
        let file_size = std::fs::metadata(file_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);

        let use_multipart = upload_bps > 0 || file_size >= LARGE_FILE_THRESHOLD;

        if use_multipart {
            self.upload_object_multipart_throttled(key, file_path, MULTIPART_PART_SIZE, upload_bps)
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
                .map_err(|e| AppError::S3(format!("PutObject failed: {}", e)))?;
        }

        Ok(())
    }

    /// Download an S3 object to a local file, applying the download rate limit
    /// when one is configured.
    pub async fn download_object(&self, key: &str, file_path: &Path) -> Result<(), AppError> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| AppError::S3(format!("GetObject failed: {}", e)))?;

        let download_bps = self.throttle.get_download_bps();

        if download_bps == 0 {
            // Unthrottled: collect entire body at once
            let data = resp
                .body
                .collect()
                .await
                .map_err(|e| AppError::S3(format!("Failed to read S3 response body: {}", e)))?;
            std::fs::write(file_path, data.into_bytes()).map_err(AppError::Io)?;
        } else {
            // Throttled: stream body in chunks and pace with sleeps
            use std::io::Write;
            let mut file = std::fs::File::create(file_path).map_err(AppError::Io)?;
            let mut body = resp.body;
            let mut bytes_done: u64 = 0;
            let mut batch_start = Instant::now();
            let mut chunk_buf: Vec<u8> = Vec::with_capacity(DOWNLOAD_CHUNK_SIZE);

            while let Some(chunk) = body.next().await {
                let bytes = chunk
                    .map_err(|e| AppError::S3(format!("Failed to read S3 response body: {}", e)))?;
                chunk_buf.extend_from_slice(&bytes);

                // Flush when we've accumulated a full chunk or at the end
                if chunk_buf.len() >= DOWNLOAD_CHUNK_SIZE {
                    let n = chunk_buf.len() as u64;
                    file.write_all(&chunk_buf).map_err(AppError::Io)?;
                    chunk_buf.clear();
                    bytes_done += n;
                    crate::throttle::enforce_rate(bytes_done, batch_start, download_bps).await;
                    bytes_done = 0;
                    batch_start = Instant::now();
                }
            }

            // Flush remainder
            if !chunk_buf.is_empty() {
                file.write_all(&chunk_buf).map_err(AppError::Io)?;
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
            .map_err(|e| AppError::S3(format!("DeleteObject failed: {}", e)))?;
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
            .map_err(|e| AppError::S3(format!("CopyObject failed: {}", e)))?;
        Ok(())
    }

    pub async fn list_objects(&self) -> Result<Vec<S3Object>, AppError> {
        let mut objects = Vec::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket);

            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| AppError::S3(format!("ListObjectsV2 failed: {}", e)))?;

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


    /// Internal: multipart upload with optional per-part throttle sleep.
    async fn upload_object_multipart_throttled(
        &self,
        key: &str,
        file_path: &Path,
        part_size: usize,
        upload_bps: u64,
    ) -> Result<(), AppError> {
        let file_data = std::fs::read(file_path)?;
        let total_size = file_data.len();

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
                .map_err(|e| AppError::S3(format!("PutObject failed: {}", e)))?;
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
            .map_err(|e| AppError::S3(format!("CreateMultipartUpload failed: {}", e)))?;

        let upload_id = create_resp
            .upload_id()
            .ok_or_else(|| AppError::S3("No upload_id returned".into()))?
            .to_string();

        let mut completed_parts = Vec::new();
        let mut part_number = 1i32;
        let mut offset = 0usize;

        let result: Result<(), AppError> = async {
            while offset < total_size {
                let end = std::cmp::min(offset + effective_part_size, total_size);
                let part_data = &file_data[offset..end];
                let part_bytes = (end - offset) as u64;

                let part_start = Instant::now();

                let resp = self
                    .client
                    .upload_part()
                    .bucket(&self.bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .part_number(part_number)
                    .body(ByteStream::from(part_data.to_vec()))
                    .send()
                    .await
                    .map_err(|e| AppError::S3(format!("UploadPart {} failed: {}", part_number, e)))?;

                let etag = resp.e_tag().unwrap_or_default().to_string();
                completed_parts.push(
                    aws_sdk_s3::types::CompletedPart::builder()
                        .part_number(part_number)
                        .e_tag(etag)
                        .build(),
                );

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
                    .map_err(|e| AppError::S3(format!("CompleteMultipartUpload failed: {}", e)))?;
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
