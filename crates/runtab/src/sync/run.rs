use std::fs::{File, OpenOptions};
use std::io::Write;

use super::token::TokenStore;
use super::{apply_sync_outcome, config, push_pull_once};
use crate::ledger::Ledger;
use crate::pricing::Pricing;

/// Self-cap for `cron.log`: past this size at open, the file is rotated to
/// `cron.log.1` (no rotation framework, just enough to stop unbounded growth).
const LOG_CAP_BYTES: u64 = 1024 * 1024;

/// One cron tick: scan, then push/pull if sync is actually ready. Unlike
/// `now()`, this never bails on logged-out/unreviewed/no-token — those are
/// quiet scan-only runs (a cron job must not spam failures for a state that
/// simply hasn't been set up yet), and it never blocks on another tick still
/// running (the lock is dropped, not held across the whole process — a crash
/// mid-tick releases it for free).
pub fn run_once(ledger: Ledger, pricing: &Pricing, verbose: bool) -> anyhow::Result<()> {
    let data_dir = config::data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let lock_file = File::create(data_dir.join("sync.lock"))?;
    if lock_file.try_lock().is_err() {
        return Ok(());
    }

    let adapters = crate::default_adapters();
    let mut summary = crate::scan(&ledger, &adapters, pricing);
    summary.rtk = crate::scan_rtk(&ledger);

    let s = ledger.sync_state()?;
    let reviewed = ledger.projects_reviewed()?;
    if !s.enabled || !reviewed {
        let reason = skip_reason(&s, reviewed, true);
        return log_line(&data_dir, &format!("skip:{reason}"), verbose);
    }
    let token = s.account_email.as_ref().and_then(|email| TokenStore::load(email).ok().flatten());
    let Some(token) = token else {
        return log_line(&data_dir, &format!("skip:{}", skip_reason(&s, reviewed, false)), verbose);
    };

    let (led, result) = push_pull_once(ledger, s.server_url.as_deref(), &token)?;

    match &result {
        Ok((pushed, pulled)) => {
            log_line(&data_dir, &format!("ok pushed={} pulled={}", pushed.pushed, pulled.pulled), verbose)?;
        }
        Err(e) => log_line(&data_dir, &format!("error:{e}"), verbose)?,
    }
    apply_sync_outcome(&led, result)
}

fn skip_reason(s: &crate::ledger::SyncState, reviewed: bool, has_token: bool) -> &'static str {
    if !s.enabled {
        "sync off"
    } else if !reviewed {
        "projects not reviewed"
    } else if !has_token {
        "no sync token"
    } else {
        "not ready"
    }
}

fn log_line(data_dir: &std::path::Path, line: &str, verbose: bool) -> anyhow::Result<()> {
    if verbose {
        println!("{line}");
    }
    let log_path = data_dir.join("cron.log");
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > LOG_CAP_BYTES {
            let _ = std::fs::rename(&log_path, data_dir.join("cron.log.1"));
        }
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&log_path)?;
    writeln!(file, "{} {line}", crate::timeutil::now_rfc3339())?;
    Ok(())
}
