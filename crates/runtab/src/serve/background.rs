use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::api::AppState;
use crate::ledger::Ledger;
use crate::pricing::Pricing;
use crate::sync::{config, pull_all, push_all, token::TokenStore, SyncClient, SyncError};

const SCAN_SECS: u64 = 30;
const PULL_EVERY_TICKS: u64 = 10; // ~5 min between pulls (spec: pull every 5 min).
const RTK_SCAN_EVERY_TICKS: u64 = 10; // ~5 min between rtk imports at the 30s cadence.

/// Rescan transcripts every 30s and, when sync is enabled, push after each scan
/// and pull periodically. A server outage only degrades sync — the local
/// dashboard keeps serving.
pub async fn run_loop(state: AppState, pricing: Arc<Pricing>) {
    let mut interval = tokio::time::interval(Duration::from_secs(SCAN_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ticks: u64 = 0;
    loop {
        interval.tick().await;
        scan_once(state.ledger.clone(), pricing.clone()).await;
        // `interval`'s first tick fires immediately (tick 0), so gating on the
        // same multiple-of-10 check that `maybe_sync` uses for pulls also
        // covers "once on startup" without a separate call.
        if ticks.is_multiple_of(RTK_SCAN_EVERY_TICKS) {
            scan_rtk_once(state.ledger.clone()).await;
        }
        maybe_sync(&state.ledger, ticks.is_multiple_of(PULL_EVERY_TICKS)).await;
        ticks = ticks.wrapping_add(1);
    }
}

/// Import + attribute rtk savings on a blocking thread, same rationale as
/// `scan_once`. `scan_rtk` already logs its own failures to stderr and
/// returns `None` when rtk isn't installed, so there's nothing to propagate
/// here — a failure must never take the rescan loop down with it.
async fn scan_rtk_once(ledger: Arc<Mutex<Ledger>>) {
    let _ = tokio::task::spawn_blocking(move || {
        let Ok(led) = ledger.lock() else {
            return;
        };
        let _ = crate::scan_rtk(&led);
    })
    .await;
}

/// Run the (blocking, file + SQLite) scan on a blocking thread so it never
/// occupies a tokio worker. The ledger lock is taken per FILE, not for the
/// whole sweep — during a large backfill, `/api` requests interleave between
/// files instead of hanging on the mutex until the entire scan finishes.
async fn scan_once(ledger: Arc<Mutex<Ledger>>, pricing: Arc<Pricing>) {
    let _ = tokio::task::spawn_blocking(move || {
        let adapters = crate::default_adapters();
        let mut summary = crate::ScanSummary::default();
        for adapter in &adapters {
            for path in adapter.discover() {
                let Ok(led) = ledger.lock() else {
                    return;
                };
                crate::scan_file(&led, adapter.as_ref(), &pricing, &path, &mut summary);
            }
        }
    })
    .await;
}

async fn maybe_sync(ledger: &Mutex<Ledger>, do_pull: bool) {
    let Some((email, server, machine_id)) = sync_target(ledger) else {
        return;
    };
    let Some(token) = TokenStore::load(&email).ok().flatten() else {
        set_degraded(ledger, "no sync token found; run `runtab sync login`");
        return;
    };
    let client = match SyncClient::new(&server) {
        Ok(c) => c,
        Err(_) => return,
    };

    let quota_reached = match push_all(ledger, &client, &token).await {
        Ok(outcome) => outcome.quota_reached,
        Err(e) => return degrade(ledger, e),
    };
    if do_pull {
        if let Err(e) = pull_all(ledger, &client, &token, &machine_id).await {
            return degrade(ledger, e);
        }
    } else if !client.healthz().await.unwrap_or(false) {
        // A push with nothing pending never contacts the server, so confirm the
        // server is actually reachable before clearing a degraded state — an
        // outage must not be masked on a no-op tick.
        return set_degraded(ledger, "sync server unreachable");
    }
    if quota_reached {
        set_quota_reached(ledger);
    } else {
        clear_degraded(ledger);
    }
}

/// (email, server_url, machine_id) when sync is enabled and configured.
fn sync_target(ledger: &Mutex<Ledger>) -> Option<(String, String, String)> {
    let led = ledger.lock().ok()?;
    let state = led.sync_state().ok()?;
    if !state.enabled {
        return None;
    }
    let email = state.account_email?;
    let server = state.server_url.unwrap_or_else(config::server_url);
    Some((email, server, led.machine_id().to_string()))
}

fn degrade(ledger: &Mutex<Ledger>, err: SyncError) {
    let msg = match err {
        SyncError::Unauthorized => {
            "sync revoked — run `runtab sync login` or `runtab sync off`".to_string()
        }
        other => other.to_string(),
    };
    set_degraded(ledger, &msg);
}

fn set_degraded(ledger: &Mutex<Ledger>, message: &str) {
    if let Ok(led) = ledger.lock() {
        let _ = led.set_degraded(true, Some(message));
    }
}

fn clear_degraded(ledger: &Mutex<Ledger>) {
    if let Ok(led) = ledger.lock() {
        let _ = led.set_degraded(false, None);
    }
}

/// Mirrors `sync::now()`'s handling of a daily-quota stop: informational, not
/// degraded, so daemon-only users still see the quota message in `sync status`
/// instead of it being erased by the next tick's `clear_degraded`.
fn set_quota_reached(ledger: &Mutex<Ledger>) {
    if let Ok(led) = ledger.lock() {
        let _ = led.set_degraded(false, Some("daily quota reached; remaining events sync on the next run"));
    }
}
