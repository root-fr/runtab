use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use runtab::adapters::{Adapter, CodexAdapter};
use runtab::ledger::Ledger;
use runtab::pricing::Pricing;
use runtab::{scan_file, ScanSummary};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn new(ext: &str) -> TempFile {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "runtab_codex_{}_{nanos}_{unique}.{ext}",
            std::process::id()
        ));
        TempFile { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, bytes: &[u8]) {
        fs::write(&self.path, bytes).unwrap();
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    fs::read(p).unwrap()
}

fn total_events(ledger: &Ledger) -> i64 {
    ledger.models(None).unwrap().iter().map(|r| r.events).sum()
}

#[test]
fn zst_parses_identically_to_plain() {
    let plain = CodexAdapter
        .parse_from(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("codex_normal.jsonl"),
            0,
        )
        .unwrap();

    // Encode the same content at test runtime and parse the .jsonl.zst copy.
    let raw = fixture_bytes("codex_normal.jsonl");
    let compressed = zstd::encode_all(&raw[..], 0).unwrap();
    let zf = TempFile::new("jsonl.zst");
    zf.write(&compressed);

    let out = CodexAdapter.parse_from(zf.path(), 0).unwrap();
    assert_eq!(out.events.len(), plain.events.len());
    for (a, b) in out.events.iter().zip(plain.events.iter()) {
        assert_eq!(a.request_id, b.request_id);
        assert_eq!(a.input_tokens, b.input_tokens);
        assert_eq!(a.cache_read_tokens, b.cache_read_tokens);
        assert_eq!(a.reasoning_tokens, b.reasoning_tokens);
        assert_eq!(a.model, b.model);
    }
    // Compressed files are immutable: new_offset is the compressed length.
    assert_eq!(out.new_offset, compressed.len() as u64);
}

#[test]
fn zst_second_parse_hits_fast_path() {
    let raw = fixture_bytes("codex_normal.jsonl");
    let compressed = zstd::encode_all(&raw[..], 0).unwrap();
    let zf = TempFile::new("jsonl.zst");
    zf.write(&compressed);
    let len = compressed.len() as u64;

    // Any non-zero offset on a .zst file skips it entirely.
    let out = CodexAdapter.parse_from(zf.path(), len).unwrap();
    assert_eq!(out.events.len(), 0);
    assert_eq!(out.new_offset, len);
}

#[test]
fn archived_path_copy_of_same_content_inserts_zero() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = CodexAdapter;
    let raw = fixture_bytes("codex_normal.jsonl");

    let active = TempFile::new("jsonl");
    active.write(&raw);
    let mut s1 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, active.path(), &mut s1);
    assert_eq!(s1.events_inserted, 2);

    // A byte-identical file at a different path (the archived_sessions rename).
    let archived = TempFile::new("jsonl");
    archived.write(&raw);
    let mut s2 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, archived.path(), &mut s2);
    // Identity is path-independent: zero new rows.
    assert_eq!(s2.events_inserted, 0);
    assert_eq!(s2.duplicates_dropped, 2);
    assert_eq!(total_events(&ledger), 2);
}

#[test]
fn unchanged_file_hits_offset_fast_path() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = CodexAdapter;
    let f = TempFile::new("jsonl");
    f.write(&fixture_bytes("codex_normal.jsonl"));

    let mut s1 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s1);
    assert_eq!(s1.events_inserted, 2);

    // Second scan: the file is unchanged, so the stored offset (== len) makes
    // parse_from take the fast path and emit nothing.
    let mut s2 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s2);
    assert_eq!(s2.events_inserted, 0);
    assert_eq!(s2.duplicates_dropped, 0);
}

#[test]
fn parse_from_at_or_past_len_is_fast_path() {
    let f = TempFile::new("jsonl");
    f.write(&fixture_bytes("codex_normal.jsonl"));
    let len = fs::metadata(f.path()).unwrap().len();

    let out = CodexAdapter.parse_from(f.path(), len).unwrap();
    assert_eq!(out.events.len(), 0);
    assert_eq!(out.new_offset, len);
}

#[test]
fn appended_file_reparses_with_zero_duplicate_inserts() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = CodexAdapter;
    let f = TempFile::new("jsonl");

    let raw = fixture_bytes("codex_normal.jsonl");
    f.write(&raw);
    let mut s1 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s1);
    assert_eq!(s1.events_inserted, 2);

    // Append a new turn_context + a genuine new token_count.
    let extra = "{\"timestamp\":\"2026-07-11T09:20:00.000Z\",\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"turn-0003\",\"cwd\":\"/home/matthieu/Documents/Dev.local/tkm\",\"approval_policy\":\"on-request\",\"sandbox_policy\":{\"read-only\":{\"network_access\":false}},\"model\":\"gpt-5.1-codex\",\"summary\":\"auto\"}}\n{\"timestamp\":\"2026-07-11T09:20:05.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":15000,\"cached_input_tokens\":5000,\"output_tokens\":2000,\"reasoning_output_tokens\":500,\"total_tokens\":17000},\"last_token_usage\":{\"input_tokens\":5350,\"cached_input_tokens\":800,\"output_tokens\":690,\"reasoning_output_tokens\":90,\"total_tokens\":6040},\"model_context_window\":272000},\"rate_limits\":null}}\n";
    let mut grown = raw.clone();
    grown.extend_from_slice(extra.as_bytes());
    f.write(&grown);

    let mut s2 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s2);
    // Full re-parse: the two old events dedup, only the new one inserts.
    assert_eq!(s2.events_inserted, 1);
    assert_eq!(s2.duplicates_dropped, 2);
    assert_eq!(total_events(&ledger), 3);
}

#[test]
fn rewritten_shorter_file_reparses_from_zero() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = CodexAdapter;
    let f = TempFile::new("jsonl");

    f.write(&fixture_bytes("codex_normal.jsonl"));
    let mut s1 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s1);
    assert_eq!(s1.events_inserted, 2);

    // Replace with a shorter, different session.
    f.write(&fixture_bytes("codex_model_switch.jsonl"));
    let mut s2 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s2);
    // The rewrite is detected (shorter file → offset reset to 0), so the new
    // session's three events land.
    assert_eq!(s2.events_inserted, 3);
    assert_eq!(total_events(&ledger), 5);
}

#[test]
fn torn_trailing_line_still_advances_offset_and_recovers_after_growth() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = CodexAdapter;
    let f = TempFile::new("jsonl");

    let raw = fixture_bytes("codex_normal.jsonl");
    // Cut the last byte off the final line: a torn trailing partial line.
    let torn = &raw[..raw.len() - 1];
    f.write(torn);
    let len_torn = torn.len() as u64;

    let mut s1 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s1);
    // The last line is a compaction re-baseline (dropped anyway); the two real
    // events still parse. Offset advances to the observed length even with a
    // torn tail (codex always re-parses on growth).
    assert_eq!(s1.events_inserted, 2);
    let stored = ledger.file_state(f.path()).unwrap().unwrap();
    assert_eq!(stored.byte_offset, len_torn);

    // File grows: the completed line is caught by the growth-triggered reparse.
    f.write(&raw);
    let mut s2 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s2);
    // The completed final line is still a zero-component re-baseline, so no new
    // event; the two prior events dedup.
    assert_eq!(s2.events_inserted, 0);
    assert_eq!(total_events(&ledger), 2);
}

#[test]
fn scanning_twice_inserts_zero_the_second_time() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = CodexAdapter;
    let f = TempFile::new("jsonl");
    f.write(&fixture_bytes("codex_normal.jsonl"));

    let mut s1 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s1);
    let mut s2 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f.path(), &mut s2);
    assert_eq!(s2.events_inserted, 0);
    assert_eq!(total_events(&ledger), 2);
}

#[test]
fn from_zero_reparse_of_same_content_inserts_zero() {
    // A from-zero re-parse (a fresh path with no stored scan state, standing in
    // for a cleared scanned_files row) must still dedup to 0 because identity is
    // content-derived, not offset- or path-derived.
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = CodexAdapter;
    let raw = fixture_bytes("codex_normal.jsonl");

    let f1 = TempFile::new("jsonl");
    f1.write(&raw);
    let mut s1 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f1.path(), &mut s1);
    assert_eq!(s1.events_inserted, 2);

    let f2 = TempFile::new("jsonl");
    f2.write(&raw);
    let mut s2 = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, f2.path(), &mut s2);
    assert_eq!(s2.events_inserted, 0);
    assert_eq!(total_events(&ledger), 2);
}

fn flagged_head() -> String {
    // A forked session_meta (forked_from_id set) flags the file for the replay
    // heuristic, plus a turn_context carrying the model.
    "{\"timestamp\":\"2026-07-11T13:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{\"session_id\":\"019105aa-0000-7000-8000-000000000001\",\"id\":\"019105aa-0000-7000-8000-000000000001\",\"forked_from_id\":\"019104a2-1e3b-7c9a-8b7d-4e2f9a1c3d5e\",\"timestamp\":\"2026-07-11T13:00:00.000Z\",\"cwd\":\"/home/u/p\",\"originator\":\"codex_cli_rs\",\"cli_version\":\"0.145.0\",\"source\":\"cli\",\"history_mode\":\"legacy\"}}\n{\"timestamp\":\"2026-07-11T13:00:00.120Z\",\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"t1\",\"cwd\":\"/home/u/p\",\"approval_policy\":\"on-request\",\"sandbox_policy\":{\"read-only\":{\"network_access\":false}},\"model\":\"gpt-5.1-codex\",\"summary\":\"auto\"}}\n".to_string()
}

fn token_count_line(sec: &str, sub: &str, cum: i64, last_in: i64, last_out: i64) -> String {
    format!(
        "{{\"timestamp\":\"{sec}.{sub}Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{cum},\"cached_input_tokens\":0,\"output_tokens\":0,\"reasoning_output_tokens\":0,\"total_tokens\":{cum}}},\"last_token_usage\":{{\"input_tokens\":{last_in},\"cached_input_tokens\":0,\"output_tokens\":{last_out},\"reasoning_output_tokens\":0,\"total_tokens\":{sum}}},\"model_context_window\":272000}},\"rate_limits\":null}}}}\n",
        sum = last_in + last_out
    )
}

#[test]
fn trailing_run_held_back_then_emitted_after_growth() {
    let adapter = CodexAdapter;
    let f = TempFile::new("jsonl");

    // A flagged file with a single genuine trailing event at its own second, and
    // NO earlier replay second: the trailing run (length 1) is held back.
    let mut first = flagged_head();
    first.push_str(&token_count_line("2026-07-11T13:00:30", "100", 1200, 1000, 200));
    f.write(first.as_bytes());

    let out1 = adapter.parse_from(f.path(), 0).unwrap();
    assert_eq!(out1.events.len(), 0, "trailing same-second run held back");

    // The file grows with a genuinely later, different-second event. The prior
    // event is no longer trailing, so it emits; the new one is now the held run.
    let mut grown = first.clone();
    grown.push_str(&token_count_line("2026-07-11T13:00:40", "000", 1920, 600, 120));
    f.write(grown.as_bytes());

    let out2 = adapter.parse_from(f.path(), 0).unwrap();
    assert_eq!(out2.events.len(), 1);
    assert_eq!(out2.events[0].ts, "2026-07-11T13:00:30.100Z");
    assert_eq!(out2.events[0].request_id, "1200");
}

#[test]
fn detected_replay_second_lets_later_trailing_run_emit() {
    let adapter = CodexAdapter;
    let f = TempFile::new("jsonl");

    // Two same-second events (a replay second) followed by a genuine trailing
    // event at a later second: the replay second is dropped and, because a
    // replay second WAS detected, the later trailing event emits normally.
    let mut body = flagged_head();
    body.push_str(&token_count_line("2026-07-11T13:00:45", "100", 100000, 40000, 3000));
    body.push_str(&token_count_line("2026-07-11T13:00:45", "500", 180000, 80000, 6000));
    body.push_str(&token_count_line("2026-07-11T13:00:52", "000", 184000, 3000, 1000));
    f.write(body.as_bytes());

    let out = adapter.parse_from(f.path(), 0).unwrap();
    assert_eq!(out.events.len(), 1);
    assert_eq!(out.events[0].request_id, "184000");
}
