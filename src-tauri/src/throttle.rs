use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// Global transfer rate limits, shared across all S3 operations.
/// Values are bytes per second; 0 means unlimited.
#[derive(Clone)]
pub struct ThrottleState {
    pub upload_bps: Arc<AtomicU64>,
    pub download_bps: Arc<AtomicU64>,
}

impl ThrottleState {
    pub fn new() -> Self {
        Self {
            upload_bps: Arc::new(AtomicU64::new(0)),
            download_bps: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn get_upload_bps(&self) -> u64 {
        self.upload_bps.load(Ordering::Relaxed)
    }

    pub fn get_download_bps(&self) -> u64 {
        self.download_bps.load(Ordering::Relaxed)
    }

    pub fn set_upload_bps(&self, bps: u64) {
        self.upload_bps.store(bps, Ordering::Relaxed);
    }

    pub fn set_download_bps(&self, bps: u64) {
        self.download_bps.store(bps, Ordering::Relaxed);
    }
}

/// Return the process-wide `ThrottleState` singleton.
///
/// All clones share the same underlying `Arc<AtomicU64>` pair, so any
/// update made via `State<ThrottleState>` in a Tauri command immediately
/// affects in-progress S3 transfers.
pub fn global() -> &'static ThrottleState {
    static GLOBAL: OnceLock<ThrottleState> = OnceLock::new();
    GLOBAL.get_or_init(ThrottleState::new)
}

/// Sleep long enough to enforce the given rate limit.
///
/// `bytes_done` is the cumulative bytes transferred in the current batch,
/// `batch_start` is when the batch started.
pub async fn enforce_rate(bytes_done: u64, batch_start: Instant, bps: u64) {
    if bps == 0 || bytes_done == 0 {
        return;
    }
    let expected = std::time::Duration::from_secs_f64(bytes_done as f64 / bps as f64);
    if let Some(sleep_for) = expected.checked_sub(batch_start.elapsed()) {
        tokio::time::sleep(sleep_for).await;
    }
}
