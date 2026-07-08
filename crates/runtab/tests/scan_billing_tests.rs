use std::path::{Path, PathBuf};

use runtab::adapters::ClaudeCodeAdapter;
use runtab::ledger::{Filter, Ledger};
use runtab::pricing::Pricing;
use runtab::{scan_file, ScanSummary};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn logged_costusd_sets_api_mode_and_preserves_cost() {
    let ledger = Ledger::open_in_memory().unwrap();
    let pricing = Pricing::load().unwrap();
    let adapter = ClaudeCodeAdapter::new();

    let mut summary = ScanSummary::default();
    scan_file(&ledger, &adapter, &pricing, &fixture("claude_logged_cost.jsonl"), &mut summary);
    assert_eq!(summary.events_inserted, 2);

    // A per-line costUSD is a real logged cost, so the source auto-detects as
    // `api` and the estimate never overwrites it.
    let s = ledger.api_summary(&Filter::default()).unwrap();
    assert_eq!(s.billing_mode, "api");
    assert_eq!(s.cost_label, "estimated spend");
    assert!(!s.plan_window_applicable);
    assert_eq!(s.est_cost_microusd, 350_000); // 0.25 + 0.10 USD, from the log

    // The plan gauge is hidden for api-mode selections.
    assert!(!ledger.api_planwindow(&Filter::default()).unwrap().applicable);
}
