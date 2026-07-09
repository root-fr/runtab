mod common;

use std::path::{Path, PathBuf};

use common::{ev, insert};
use runtab::adapters::ClaudeCodeAdapter;
use runtab::ledger::Ledger;
use runtab::model::CostBasis::Estimated;
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
    let rows = ledger.models(None).unwrap();
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

    let rows = ledger.daily(None).unwrap();
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
    assert_eq!(ledger.projects(None).unwrap().len(), 2);
    assert_eq!(ledger.sessions(None).unwrap().len(), 2);
    assert_eq!(ledger.daily(None).unwrap().len(), 2);

    let opus = ledger
        .models(None)
        .unwrap()
        .into_iter()
        .find(|r| r.key == "claude-opus-4-8")
        .unwrap();
    assert!(opus.cost_usd.is_some());
}

#[test]
fn aggregates_filter_by_since_date() {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev("m1", "s1", "/p", "model-x", "2026-06-01T10:00:00Z", 100, Estimated, Some(1.0)));
    insert(&l, &ev("m2", "s2", "/p", "model-x", "2026-07-01T10:00:00Z", 200, Estimated, Some(2.0)));

    assert_eq!(l.daily(None).unwrap().len(), 2);
    let recent = l.daily(Some("2026-06-15")).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].key, "2026-07-01");
    assert_eq!(l.sessions(Some("2026-06-15")).unwrap().len(), 1);
    assert_eq!(l.models(Some("2026-06-15")).unwrap()[0].events, 1);
    assert!(l.projects(Some("2027-01-01")).unwrap().is_empty());
}

#[test]
fn totals_rolls_up_whole_ledger_and_respects_since() {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev("m1", "s1", "/p", "model-x", "2026-06-01T10:00:00Z", 100, Estimated, Some(1.0)));
    insert(&l, &ev("m2", "s1", "/p", "model-x", "2026-06-02T10:00:00Z", 100, Estimated, Some(1.0)));
    insert(&l, &ev("m3", "s2", "/p", "model-x", "2026-07-01T10:00:00Z", 200, Estimated, Some(2.0)));

    let all = l.totals(None).unwrap();
    assert_eq!(all.events, 3);
    assert_eq!(all.total_tokens, 400);
    assert_eq!(all.cost_usd, Some(4.0));
    assert_eq!(all.unpriced_events, 0);
    assert_eq!(all.sessions, 2);
    assert_eq!(all.first_day.as_deref(), Some("2026-06-01"));

    let windowed = l.totals(Some("2026-06-15")).unwrap();
    assert_eq!(windowed.events, 1);
    assert_eq!(windowed.sessions, 1);
    assert_eq!(windowed.first_day.as_deref(), Some("2026-07-01"));

    let empty = l.totals(Some("2027-01-01")).unwrap();
    assert_eq!(empty.events, 0);
    assert!(empty.first_day.is_none());
    assert!(empty.cost_usd.is_none());
}
