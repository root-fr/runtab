use std::sync::Mutex;
use std::time::Duration;

use super::client::{SyncClient, SyncError};
use crate::ledger::Ledger;

/// Batch cap from the hardening addendum (1,000 events / request).
const BATCH: i64 = 1000;

/// Consecutive rate-limit waits allowed for a single push batch or pull page
/// before giving up. At the server's real Retry-After (≤60s) this is up to
/// ~40 minutes of patience — enough to ride out a large backfill without
/// hanging forever on a wedged batch. Shared with `pull.rs`.
pub(super) const MAX_RATE_WAITS: u32 = 40;

#[derive(Debug, Default)]
pub struct PushOutcome {
    pub pushed: u64,
    pub batches: u64,
    /// Set when the server's daily quota was reached; the push cursor stops
    /// at the last accepted batch and the rest resumes on a later run.
    pub quota_reached: bool,
    /// Seconds until the daily quota resets (0 when `quota_reached` is false).
    pub quota_retry_after: u64,
}

/// Push every unpushed local row in batches, advancing the push cursor only
/// after the server accepts a batch. Never holds the ledger lock across an
/// `await`: rows are read into an owned batch, sent, then the cursor is written.
///
/// Honors the server's throttle signals: a per-minute rate limit pauses and
/// retries the same batch, while a daily quota stops the loop cleanly (not
/// an error) so the remainder syncs on a later run.
pub async fn push_all(
    ledger: &Mutex<Ledger>,
    client: &SyncClient,
    token: &str,
) -> Result<PushOutcome, SyncError> {
    let mut out = PushOutcome::default();
    loop {
        let batch = {
            let l = lock(ledger)?;
            l.pending_batch(BATCH).map_err(local)?
        };
        // `scanned == 0` means either the cursor is drained or the projects have
        // not been reviewed yet (consent gate) — either way, nothing to send.
        if batch.scanned == 0 {
            break;
        }
        if !batch.records.is_empty() {
            let mut rate_waits = 0;
            loop {
                match client.push_events(token, &batch.records).await {
                    Ok(result) => {
                        out.pushed += result.accepted;
                        out.batches += 1;
                        break;
                    }
                    Err(SyncError::RateLimited { retry_after }) => {
                        rate_waits += 1;
                        if rate_waits > MAX_RATE_WAITS {
                            return Err(SyncError::RateLimited { retry_after });
                        }
                        eprintln!(
                            "runtab: rate limited by server, pausing {retry_after}s… ({} events synced so far)",
                            out.pushed
                        );
                        tokio::time::sleep(Duration::from_secs(retry_after.min(120))).await;
                    }
                    Err(SyncError::QuotaDaily { retry_after }) => {
                        out.quota_reached = true;
                        out.quota_retry_after = retry_after;
                        return Ok(out);
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        // Advance past every scanned row (including excluded ones) so an excluded
        // project never blocks the cursor behind it, and clear the re-push flag on
        // every dirty row now that the server holds >= our totals (a duplicate
        // count still means the server already has the row). Clearing excluded
        // rows too stops them re-scanning every batch.
        {
            let l = lock(ledger)?;
            l.set_last_pushed_id(batch.max_id).map_err(local)?;
            l.clear_dirty(&batch.dirty_ids).map_err(local)?;
        }
        if batch.scanned < BATCH as usize {
            break;
        }
    }
    Ok(out)
}

pub(super) fn lock(ledger: &Mutex<Ledger>) -> Result<std::sync::MutexGuard<'_, Ledger>, SyncError> {
    ledger.lock().map_err(|_| SyncError::Local("ledger lock poisoned".to_string()))
}

pub(super) fn local(e: rusqlite::Error) -> SyncError {
    SyncError::Local(e.to_string())
}
