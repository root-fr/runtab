use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use runtab::adapters::ClaudeCodeAdapter;
use runtab::ledger::Ledger;
use runtab::pricing::Pricing;
use runtab::{scan_file, ScanSummary};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn temp_db() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "runtab_toolevt_db_{}_{nanos}_{unique}.db",
        std::process::id()
    ))
}

fn tool_events_count(path: &Path) -> i64 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM tool_events", [], |r| r.get(0))
        .unwrap()
}

fn pending_count(path: &Path) -> i64 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM pending_tool_calls", [], |r| r.get(0))
        .unwrap()
}

/// A temp transcript file that removes itself on drop, mirroring the helper
/// in `incremental_tests.rs`.
struct TempTranscript {
    path: PathBuf,
}

impl TempTranscript {
    fn new() -> TempTranscript {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "runtab_toolevt_{}_{nanos}_{unique}.jsonl",
            std::process::id()
        ));
        TempTranscript { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, bytes: &[u8]) {
        std::fs::write(&self.path, bytes).unwrap();
    }

    fn append(&self, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .unwrap();
        f.write_all(bytes).unwrap();
    }
}

impl Drop for TempTranscript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn scan_file_pairs_same_batch_use_and_result_and_leaves_dangling_use_pending() {
    let db_path = temp_db();
    let ledger = Ledger::open(&db_path).unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();

    let mut summary = ScanSummary::default();
    scan_file(
        &ledger,
        &adapter,
        &pricing,
        &fixture("claude_tool_events.jsonl"),
        &mut summary,
    );

    // toolu_100 (Bash, use on line 1 + result on line 3) pairs into one row.
    assert_eq!(summary.tool_events_inserted, 1);
    assert_eq!(tool_events_count(&db_path), 1);
    // toolu_101 (Read, use only on line 4) has no result in this file yet.
    assert_eq!(pending_count(&db_path), 1);
    assert_eq!(summary.db_errors, 0);

    drop(ledger);
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn cross_scan_pairing_resolves_a_use_staged_in_an_earlier_scan() {
    let db_path = temp_db();
    let ledger = Ledger::open(&db_path).unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();
    let f = TempTranscript::new();

    let use_line = "{\"type\":\"assistant\",\"timestamp\":\"2026-07-06T10:00:00.000Z\",\
         \"sessionId\":\"sx\",\"cwd\":\"/home/u/p\",\
         \"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_x\",\
         \"name\":\"Read\",\"input\":{}}]}}\n";
    f.write(use_line.as_bytes());

    let mut first = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut first);
    assert_eq!(first.tool_events_inserted, 0);
    assert_eq!(pending_count(&db_path), 1);
    assert_eq!(tool_events_count(&db_path), 0);

    let result_line = "{\"type\":\"user\",\"timestamp\":\"2026-07-06T10:00:05.000Z\",\
         \"sessionId\":\"sx\",\"cwd\":\"/home/u/p\",\
         \"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_x\",\"content\":\"ok\"}]}}\n";
    f.append(result_line.as_bytes());

    let mut second = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut second);
    assert_eq!(second.tool_events_inserted, 1);
    assert_eq!(tool_events_count(&db_path), 1);
    assert_eq!(pending_count(&db_path), 0);

    drop(ledger);
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn replaying_the_same_content_after_an_offset_reset_does_not_duplicate() {
    let db_path = temp_db();
    let ledger = Ledger::open(&db_path).unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();
    let f = TempTranscript::new();

    let content = "{\"type\":\"assistant\",\"timestamp\":\"2026-07-06T10:00:00.000Z\",\
         \"sessionId\":\"sy\",\"cwd\":\"/home/u/p\",\
         \"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_y\",\
         \"name\":\"Read\",\"input\":{}}]}}\n\
         {\"type\":\"user\",\"timestamp\":\"2026-07-06T10:00:05.000Z\",\
         \"sessionId\":\"sy\",\"cwd\":\"/home/u/p\",\
         \"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_y\",\"content\":\"ok\"}]}}\n";
    f.write(content.as_bytes());

    let mut first = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut first);
    assert_eq!(first.tool_events_inserted, 1);
    assert_eq!(tool_events_count(&db_path), 1);
    assert_eq!(pending_count(&db_path), 0);

    // Force an offset reset without changing content: back-date the file's
    // mtime so `resume_offset` sees `mtime < stored.mtime` and re-reads from
    // byte 0, replaying the exact same use/result pair (same mechanism as a
    // partial-write rewrite, deterministic — no reliance on nanosecond mtime
    // jitter between two `fs::write` calls).
    let backdated = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::open(f.path())
        .unwrap()
        .set_modified(backdated)
        .unwrap();

    let mut second = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut second);
    assert_eq!(
        second.tool_events_inserted, 0,
        "the replayed pair must not be counted as a fresh insert"
    );
    assert_eq!(
        tool_events_count(&db_path),
        1,
        "exactly one tool_events row must survive the replay"
    );
    assert_eq!(
        pending_count(&db_path),
        0,
        "the pending row must not be resurrected"
    );
    assert_eq!(second.db_errors, 0);

    drop(ledger);
    let _ = std::fs::remove_file(&db_path);
}
