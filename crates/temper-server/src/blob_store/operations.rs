use std::time::{Duration, Instant};

use super::limits::{BLOB_BUFFERED_OPERATION_TIMEOUT, BLOB_IO_QUEUE_TIMEOUT, blob_io_semaphore};
use super::{BlobStore, BlobStoreBackend, local};

impl BlobStore {
    /// Write bytes to a content-addressed key.
    ///
    /// Remote object stores use a direct `PUT` because the key is already
    /// derived from the payload hash; avoiding a preflight existence check
    /// saves a round trip on the File `$value` write path.
    pub(crate) async fn put_content_addressed(
        &self,
        key: &str,
        body: &[u8],
        ttl: Option<Duration>,
    ) -> Result<(), String> {
        let queued_at = Instant::now(); // determinism-ok: production blob I/O queue metric only
        let _permit =
            tokio::time::timeout(BLOB_IO_QUEUE_TIMEOUT, blob_io_semaphore().acquire_owned())
                .await
                .map_err(|_| "content-addressed blob put queue deadline exceeded".to_string())?
                .expect("blob semaphore closed"); // ci-ok: process-global and never closed
        let wait_duration = queued_at.elapsed();
        crate::runtime_metrics::record_blob_io_wait_duration(wait_duration, "put_content");
        if wait_duration.as_millis() > 0 {
            tracing::info!(path = %key, wait_ms = wait_duration.as_millis() as u64, "content-addressed blob put queued");
        }
        if ttl.is_some() {
            tracing::debug!(
                path = %key,
                ttl_seconds = ttl.map(|duration| duration.as_secs()),
                "content-addressed blob write received TTL; retention is delegated to the object store"
            );
        }

        tokio::time::timeout(BLOB_BUFFERED_OPERATION_TIMEOUT, async {
            match &self.backend {
                BlobStoreBackend::LocalFs { root } => {
                    local::put_local_blob_replace_observed(root, key, body, "put_content").await
                }
                BlobStoreBackend::S3(store) => {
                    store.put_with_operation("put_content", key, body).await
                }
            }
        })
        .await
        .map_err(|_| format!("content-addressed blob put timed out for '{key}'"))?
    }

    pub(crate) async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let queued_at = Instant::now(); // determinism-ok: production blob I/O queue metric only
        let _permit =
            tokio::time::timeout(BLOB_IO_QUEUE_TIMEOUT, blob_io_semaphore().acquire_owned())
                .await
                .map_err(|_| "blob get queue deadline exceeded".to_string())?
                .expect("blob semaphore closed"); // ci-ok: process-global and never closed
        let wait_duration = queued_at.elapsed();
        crate::runtime_metrics::record_blob_io_wait_duration(wait_duration, "get");
        if wait_duration.as_millis() > 0 {
            tracing::info!(path = %key, wait_ms = wait_duration.as_millis() as u64, "blob get queued");
        }

        tokio::time::timeout(BLOB_BUFFERED_OPERATION_TIMEOUT, async {
            match &self.backend {
                BlobStoreBackend::LocalFs { root } => {
                    local::get_local_blob_observed(root, key).await
                }
                BlobStoreBackend::S3(store) => store.get(key).await,
            }
        })
        .await
        .map_err(|_| format!("blob get timed out for '{key}'"))?
    }
}
