use std::sync::Mutex;
use std::time::Duration;

use super::client::{SyncClient, SyncError};
use super::push::{local, lock, MAX_RATE_WAITS};
use crate::ledger::Ledger;

#[derive(Debug, Default)]
pub struct PullOutcome {
    pub pulled: u64,
}

/// Pull other machines' rows into the local merged store, following the
/// `server_seq` cursor until the server reports no more. `exclude_machine` is
/// this machine's id so rows it already holds locally are never re-fetched.
///
/// Honors the server's per-minute `EventsGet` rate limit: a 429 pauses and
/// retries the SAME page (the cursor only advances after a page is actually
/// applied), so a rate limit partway through a large first pull never skips
/// remote events. `QuotaDaily` never applies to GET — only `POST /v1/events`
/// is quota-limited — so it is propagated like any other error here.
pub async fn pull_all(
    ledger: &Mutex<Ledger>,
    client: &SyncClient,
    token: &str,
    exclude_machine: &str,
) -> Result<PullOutcome, SyncError> {
    let mut out = PullOutcome::default();
    loop {
        let cursor = {
            let l = lock(ledger)?;
            l.sync_state().map_err(local)?.pull_cursor
        };
        let resp = {
            let mut rate_waits = 0;
            loop {
                match client.pull_events(token, cursor, exclude_machine).await {
                    Ok(resp) => break resp,
                    Err(SyncError::RateLimited { retry_after }) => {
                        rate_waits += 1;
                        if rate_waits > MAX_RATE_WAITS {
                            return Err(SyncError::RateLimited { retry_after });
                        }
                        eprintln!(
                            "runtab: rate limited by server, pausing {retry_after}s… ({} events pulled so far)",
                            out.pulled
                        );
                        tokio::time::sleep(Duration::from_secs(retry_after.min(120))).await;
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        {
            let l = lock(ledger)?;
            for row in &resp.events {
                l.upsert_remote(row).map_err(local)?;
            }
            l.set_pull_cursor(resp.next_since).map_err(local)?;
        }
        out.pulled += resp.events.len() as u64;
        // Stop when the server is drained or the cursor fails to advance (guards
        // against a server that keeps reporting has_more without progress).
        if !resp.has_more || resp.next_since <= cursor {
            break;
        }
    }
    Ok(out)
}
