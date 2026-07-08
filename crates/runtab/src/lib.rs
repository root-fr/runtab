pub mod adapters;
pub mod billing;
pub mod cmdnorm;
pub mod encoding;
pub mod ledger;
pub mod model;
pub mod pricing;
pub mod report;
pub mod rtkimport;
pub mod serve;
pub mod sync;
pub mod timeutil;
pub mod wire;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use adapters::{Adapter, ClaudeCodeAdapter, CodexAdapter};
use ledger::{Ledger, UpsertResult};
use pricing::Pricing;

/// The adapter set every scan path (CLI `scan`, cron `sync run`, `serve`'s
/// background loop) runs against.
pub fn default_adapters() -> Vec<Box<dyn Adapter>> {
    vec![Box::new(ClaudeCodeAdapter::new()), Box::new(CodexAdapter)]
}

/// Home directory from `$HOME` (or `%USERPROFILE%`), no external crate.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

/// Summary of one scan run, printed by `runtab scan`.
#[derive(Debug, Default, Serialize)]
pub struct ScanSummary {
    pub files_scanned: u64,
    pub events_inserted: u64,
    pub duplicates_dropped: u64,
    pub lines_skipped: u64,
    pub db_errors: u64,
    pub tool_events_inserted: u64,
    /// `tool_use` blocks still awaiting a result — a backlog signal, not an
    /// error (see `Ledger::pending_tool_calls_count`).
    pub pending_tool_calls: u64,
    pub unknown_models: BTreeSet<String>,
    /// rtk savings imported/attributed this run. `None` whenever rtk isn't
    /// installed on this machine, so JSON output is unchanged when the
    /// feature is absent (see `scan_rtk`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtk: Option<RtkReport>,
}

/// rtk savings numbers surfaced alongside a scan (see `scan_rtk`).
#[derive(Debug, Serialize)]
pub struct RtkReport {
    pub rows_imported: u64,
    pub attributed_text: u64,
    pub attributed_window: u64,
    pub unmatched: u64,
}

/// How long a staged `tool_use` waits for its `tool_result` before it's
/// considered abandoned (interrupted session, rotated transcript).
const PENDING_TOOL_CALL_MAX_AGE_SECS: i64 = 30 * 86_400;

/// Idempotent scan across all adapters. Re-scanning is always safe: the
/// per-file byte offset skips already-read bytes and the dedup key drops
/// replays.
pub fn scan(ledger: &Ledger, adapters: &[Box<dyn Adapter>], pricing: &Pricing) -> ScanSummary {
    let mut summary = ScanSummary::default();
    for adapter in adapters {
        for path in adapter.discover() {
            scan_file(ledger, adapter.as_ref(), pricing, &path, &mut summary);
        }
    }
    let cutoff = timeutil::epoch_to_rfc3339(timeutil::now_epoch() - PENDING_TOOL_CALL_MAX_AGE_SECS);
    if let Err(e) = ledger.prune_pending(&cutoff) {
        eprintln!("runtab: cannot prune pending tool calls: {e}");
    }
    match ledger.pending_tool_calls_count() {
        Ok(n) => summary.pending_tool_calls = n,
        Err(e) => eprintln!("runtab: cannot count pending tool calls: {e}"),
    }
    summary
}

/// Runs the rtk import + attribution phase after the adapter scan (see
/// `rtkimport`). Absent rtk db or an import failure must not fail the scan
/// itself, which has already succeeded by the time this runs: both are
/// logged to stderr and treated as "rtk not available" (`None`).
///
/// A failure in attribution is different: the import already committed, so
/// `rtk_events` rows exist and `rtk_totals`/`SAVED` columns are no longer
/// truly absent. Reporting `None` there would make a real import look like
/// rtk isn't installed. Instead this logs the error and returns `Some`
/// with the real import counts and attribution counts zeroed; the rows stay
/// `match_kind = 'none'` and the next scan's watermark-driven `attribute`
/// picks them back up, so nothing is lost, only delayed.
pub fn scan_rtk(ledger: &Ledger) -> Option<RtkReport> {
    let import_summary = match rtkimport::run(ledger) {
        Ok(Some(summary)) => summary,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("runtab: rtk import skipped: {e:#}");
            return None;
        }
    };

    match rtkimport::attribute(ledger) {
        Ok(attribution) => Some(RtkReport {
            rows_imported: import_summary.rows_imported,
            attributed_text: attribution.text,
            attributed_window: attribution.window,
            unmatched: attribution.none,
        }),
        Err(e) => {
            eprintln!("runtab: rtk attribution failed (will retry next scan): {e:#}");
            Some(RtkReport {
                rows_imported: import_summary.rows_imported,
                attributed_text: 0,
                attributed_window: 0,
                unmatched: 0,
            })
        }
    }
}

/// Scan a single file, resuming from its stored byte offset. I/O errors on one
/// file are logged and skipped; the scan continues.
pub fn scan_file(
    ledger: &Ledger,
    adapter: &dyn Adapter,
    pricing: &Pricing,
    path: &Path,
    summary: &mut ScanSummary,
) {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("runtab: cannot stat {}: {e}", path.display());
            return;
        }
    };
    let len = meta.len();
    let mtime = mtime_nanos(&meta);
    let stored = match ledger.file_state(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("runtab: cannot read scan state for {}: {e}", path.display());
            None
        }
    };
    let offset = resume_offset(stored.as_ref(), len, mtime);

    let parsed = match adapter.parse_from(path, offset) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("runtab: cannot read {}: {e}", path.display());
            return;
        }
    };

    summary.files_scanned += 1;
    summary.lines_skipped += parsed.lines_skipped;

    // One transaction per file: batching the upserts costs one WAL commit
    // instead of one per event, and makes the events + offset advance atomic.
    let tx = ledger.tx_begin().is_ok();
    let mut db_error = false;
    for mut event in parsed.events {
        pricing.apply(&mut event, &mut summary.unknown_models);
        match ledger.upsert(&event) {
            Ok(UpsertResult::Inserted) => summary.events_inserted += 1,
            Ok(_) => summary.duplicates_dropped += 1,
            Err(e) => {
                eprintln!("runtab: db error on {}: {e}", event.message_id);
                summary.db_errors += 1;
                db_error = true;
            }
        }
    }

    // All tool_uses staged before any tool_results resolve: a result always
    // follows its use within one file, so same-batch pairs work.
    for tool_use in &parsed.tool_uses {
        if let Err(e) = ledger.insert_pending_tool_use(tool_use) {
            eprintln!("runtab: db error on tool_use {}: {e}", tool_use.tool_use_id);
            summary.db_errors += 1;
            db_error = true;
        }
    }
    for tool_result in &parsed.tool_results {
        match ledger.resolve_tool_result(tool_result) {
            Ok(true) => summary.tool_events_inserted += 1,
            Ok(false) => {}
            Err(e) => {
                eprintln!("runtab: db error on tool_result {}: {e}", tool_result.tool_use_id);
                summary.db_errors += 1;
                db_error = true;
            }
        }
    }

    // A failed upsert must not let the offset advance past the event it lost.
    // Leave the file's scan state untouched so the next scan re-reads and
    // retries; dedup drops the rows that did land (or the rollback removes them).
    if db_error {
        if tx {
            let _ = ledger.tx_rollback();
        }
        return;
    }
    if let Err(e) = ledger.set_file_state(path, len, mtime, parsed.new_offset) {
        eprintln!("runtab: cannot record scan state for {}: {e}", path.display());
        if tx {
            let _ = ledger.tx_rollback();
        }
        return;
    }
    if tx {
        if let Err(e) = ledger.tx_commit() {
            eprintln!("runtab: db error committing {}: {e}", path.display());
            summary.db_errors += 1;
            let _ = ledger.tx_rollback();
        }
    }
}

/// Byte offset to resume the next scan from, or 0 to re-read the whole file.
/// JSONL transcripts are append-only, so we resume only when the file looks like
/// a plain extension of what we already read. A shorter file, a same-length
/// change, or a backdated mtime all indicate a rewrite/rotation whose stored
/// offset would otherwise splice us into the middle of new content.
fn resume_offset(stored: Option<&ledger::FileState>, len: u64, mtime: i64) -> u64 {
    let Some(s) = stored else {
        return 0;
    };
    if len < s.size || len < s.byte_offset || mtime < s.mtime {
        return 0;
    }
    if len == s.size && mtime != s.mtime {
        return 0;
    }
    s.byte_offset
}

/// Nanosecond mtime, so a rewrite in the same wall-clock second as the last
/// scan still reads as a change (seconds resolution would miss it).
fn mtime_nanos(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}
