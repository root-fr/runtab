use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use runtab::adapters::{Adapter, ClaudeCodeAdapter};
use runtab::ledger::Ledger;
use runtab::pricing::Pricing;
use runtab::{scan_file, ScanSummary};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp transcript file that removes itself on drop.
struct TempTranscript {
    path: PathBuf,
}

impl TempTranscript {
    fn new() -> TempTranscript {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "runtab_it_{}_{nanos}_{unique}.jsonl",
            std::process::id()
        ));
        TempTranscript { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, bytes: &[u8]) {
        fs::write(&self.path, bytes).unwrap();
    }
}

impl Drop for TempTranscript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn line(id: &str, req: &str, input: i64) -> String {
    format!(
        "{{\"type\":\"assistant\",\"timestamp\":\"2026-07-05T15:00:00.000Z\",\
         \"sessionId\":\"s1\",\"requestId\":\"{req}\",\"uuid\":\"u-{id}\",\
         \"cwd\":\"/home/u/p\",\"version\":\"1.0.0\",\
         \"message\":{{\"id\":\"{id}\",\"model\":\"claude-opus-4-8\",\
         \"usage\":{{\"input_tokens\":{input},\"output_tokens\":5}}}}}}"
    )
}

#[test]
fn partial_trailing_line_is_deferred_then_parsed() {
    let f = TempTranscript::new();
    let complete = line("mG", "rG", 10);
    let split = 120.min(complete.len() - 1);

    // Writer is mid-append: the record is only partially flushed, no newline.
    f.write(&complete.as_bytes()[..split]);
    let first = ClaudeCodeAdapter::new()
        .parse_from(f.path(), 0)
        .unwrap();
    assert_eq!(first.events.len(), 0);
    assert_eq!(first.new_offset, 0, "offset must stay before the partial line");

    // Writer finishes the line. Re-scanning from the deferred offset recovers it.
    f.write(format!("{complete}\n").as_bytes());
    let second = ClaudeCodeAdapter::new()
        .parse_from(f.path(), first.new_offset)
        .unwrap();
    assert_eq!(second.events.len(), 1);
    assert_eq!(second.events[0].message_id, "mG");
}

fn tool_use_line(id: &str) -> String {
    format!(
        "{{\"type\":\"assistant\",\"timestamp\":\"2026-07-05T15:00:00.000Z\",\
         \"sessionId\":\"s1\",\"cwd\":\"/home/u/p\",\
         \"message\":{{\"content\":[{{\"type\":\"tool_use\",\"id\":\"{id}\",\
         \"name\":\"Read\",\"input\":{{}}}}]}}}}"
    )
}

#[test]
fn partial_tool_use_line_is_deferred_then_parsed() {
    let f = TempTranscript::new();
    let complete = tool_use_line("toolu_1");
    let split = 60.min(complete.len() - 1);

    // Writer is mid-append: the tool_use record is only partially flushed, no newline.
    f.write(&complete.as_bytes()[..split]);
    let first = ClaudeCodeAdapter::new()
        .parse_from(f.path(), 0)
        .unwrap();
    assert!(first.tool_uses.is_empty());
    assert_eq!(first.new_offset, 0, "offset must stay before the partial line");

    // Writer finishes the line. Re-scanning from the deferred offset recovers it.
    f.write(format!("{complete}\n").as_bytes());
    let second = ClaudeCodeAdapter::new()
        .parse_from(f.path(), first.new_offset)
        .unwrap();
    assert_eq!(second.tool_uses.len(), 1);
    assert_eq!(second.tool_uses[0].tool_use_id, "toolu_1");
}

#[test]
fn tool_use_scan_recovers_line_completed_after_a_prior_scan() {
    let f = TempTranscript::new();
    let prior = format!("{}\n", tool_use_line("toolu_prior"));
    let next = tool_use_line("toolu_next");
    let split = 60.min(next.len() - 1);

    // First scan sees a complete tool_use line followed by a partial one.
    f.write(format!("{prior}{}", &next[..split]).as_bytes());
    let first = ClaudeCodeAdapter::new()
        .parse_from(f.path(), 0)
        .unwrap();
    assert_eq!(first.tool_uses.len(), 1);
    assert_eq!(first.tool_uses[0].tool_use_id, "toolu_prior");

    // The partial line is completed; resuming from the stored offset must
    // pick it up, without re-seeing the prior line.
    f.write(format!("{prior}{next}\n").as_bytes());
    let second = ClaudeCodeAdapter::new()
        .parse_from(f.path(), first.new_offset)
        .unwrap();
    assert_eq!(second.tool_uses.len(), 1);
    assert_eq!(second.tool_uses[0].tool_use_id, "toolu_next");
}

#[test]
fn invalid_utf8_line_is_skipped_not_fatal() {
    let f = TempTranscript::new();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(line("mD", "rD", 1).as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(&[0xFF, 0xFE, 0xFD]); // not valid UTF-8
    bytes.push(b'\n');
    bytes.extend_from_slice(line("mE", "rE", 2).as_bytes());
    bytes.push(b'\n');
    f.write(&bytes);

    let out = ClaudeCodeAdapter::new()
        .parse_from(f.path(), 0)
        .unwrap();

    // Both valid events survive; the undecodable line is counted, not fatal.
    assert_eq!(out.events.len(), 2);
    assert_eq!(out.lines_skipped, 1);
    assert_eq!(out.new_offset, bytes.len() as u64);
}

#[test]
fn scan_recovers_line_completed_after_a_prior_scan() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();
    let f = TempTranscript::new();

    let mf = format!("{}\n", line("mF", "rF", 10));
    let mg = line("mG", "rG", 20);
    let split = 100.min(mg.len() - 1);

    // First scan sees a complete record followed by a partial one.
    f.write(format!("{mf}{}", &mg[..split]).as_bytes());
    let mut first = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut first);
    assert_eq!(first.events_inserted, 1);

    // The partial record is completed; the next scan must not have skipped it.
    f.write(format!("{mf}{mg}\n").as_bytes());
    let mut second = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut second);
    assert_eq!(second.events_inserted, 1);

    let total: i64 = ledger.models().unwrap().iter().map(|r| r.events).sum();
    assert_eq!(total, 2, "both mF and mG must be in the ledger");
}

#[test]
fn rewritten_shorter_file_is_reingested_from_the_start() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();
    let f = TempTranscript::new();

    // A long record, then the file is replaced with a different, shorter one.
    f.write(format!("{}\n", line("mA", "rA", 999_999)).as_bytes());
    let mut first = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut first);
    assert_eq!(first.events_inserted, 1);

    f.write(format!("{}\n", line("mB", "rB", 1)).as_bytes());
    let mut second = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut second);

    // The rewrite is detected (file shrank), so mB is read rather than the
    // scan resuming past the shorter content and losing it.
    assert_eq!(second.events_inserted, 1);
    let events: i64 = ledger.models().unwrap().iter().map(|r| r.events).sum();
    assert_eq!(events, 2, "mB must be ingested after the rewrite");
}
