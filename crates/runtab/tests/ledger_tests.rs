use std::path::{Path, PathBuf};

use runtab::adapters::ClaudeCodeAdapter;
use runtab::ledger::Ledger;
use runtab::pricing::Pricing;
use runtab::{scan_file, ScanSummary};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn dedup_keeps_higher_total_tokens() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();

    let mut summary = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, &fixture("claude_dup.jsonl"), &mut summary);

    assert_eq!(summary.events_inserted, 1);
    assert_eq!(summary.duplicates_dropped, 1);

    // The surviving row must be the higher-total replay: 100+200+50 = 350.
    let rows = ledger.models().unwrap();
    let total: i64 = rows.iter().map(|r| r.total_tokens).sum();
    assert_eq!(total, 350);
}

#[test]
fn second_scan_of_same_file_inserts_zero() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();
    let path = fixture("claude_normal.jsonl");

    let mut first = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, &path, &mut first);
    assert_eq!(first.events_inserted, 3);

    let mut second = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, &path, &mut second);
    assert_eq!(second.events_inserted, 0);
    assert_eq!(second.duplicates_dropped, 0);
    assert_eq!(second.files_scanned, 1);
}

#[test]
fn unknown_model_stored_with_null_cost() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();

    let mut summary = ScanSummary::default();
    scan_file(
        &ledger,
        &adapter,
        &pricing,
        &fixture("claude_unknown_model.jsonl"),
        &mut summary,
    );

    assert_eq!(summary.events_inserted, 1);
    assert!(summary.unknown_models.contains("mystery-model-1"));

    let rows = ledger.daily().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows.iter().all(|r| r.cost_usd.is_none()));
    // The unpriced event is surfaced in the aggregate so a partial COST(est) is
    // never mistaken for the group's full cost.
    assert_eq!(rows[0].unpriced_events, 1);
}

#[test]
fn aggregates_group_by_project_and_session() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();

    let mut summary = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, &fixture("claude_normal.jsonl"), &mut summary);

    // Two projects (projA, projB), two sessions (s1, s2), two calendar days.
    assert_eq!(ledger.projects().unwrap().len(), 2);
    assert_eq!(ledger.sessions().unwrap().len(), 2);
    assert_eq!(ledger.daily().unwrap().len(), 2);

    let opus = ledger
        .models()
        .unwrap()
        .into_iter()
        .find(|r| r.key == "claude-opus-4-8")
        .unwrap();
    assert!(opus.cost_usd.is_some());
}
