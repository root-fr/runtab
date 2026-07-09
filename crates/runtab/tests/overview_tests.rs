mod common;

use common::{ev, insert};
use runtab::format::Style;
use runtab::ledger::Ledger;
use runtab::model::CostBasis::Estimated;

#[test]
fn overview_on_empty_ledger_points_at_log_locations() {
    let l = Ledger::open_in_memory().unwrap();
    let out = runtab::overview::render(&l, &Style::new(false)).unwrap();
    assert!(out.contains("No agent usage found"));
    assert!(out.contains(".claude/projects"));
    assert!(out.contains("runtab --help"));
    assert!(!out.contains("Last 30 days"));
}

#[test]
fn overview_shows_window_today_and_top_lines() {
    let l = Ledger::open_in_memory().unwrap();
    let now = runtab::timeutil::now_rfc3339();
    insert(&l, &ev("m1", "s1", "/home/u/projA", "model-x", &now, 1_000_000, Estimated, Some(2.0)));
    insert(&l, &ev("m2", "s2", "/home/u/projB", "model-y", &now, 3_000_000, Estimated, Some(6.0)));

    let out = runtab::overview::render(&l, &Style::new(false)).unwrap();
    assert!(out.contains("Last 30 days   $8.00 est · 4M tokens · 2 sessions"));
    assert!(out.contains("Today          $8.00 est · 4M tokens · 2 sessions"));
    assert!(out.contains("Top model      model-y · $6.00 (75%)"));
    assert!(out.contains("Top project    projB · $6.00 (75%)"));
    assert!(out.contains("runtab daily"));
    assert!(out.contains("runtab serve"));
}

#[test]
fn overview_with_old_data_only_shows_quiet_window() {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev("m1", "s1", "/p", "model-x", "2020-01-01T10:00:00Z", 100, Estimated, Some(1.0)));
    let out = runtab::overview::render(&l, &Style::new(false)).unwrap();
    assert!(out.contains("Last 30 days   —"));
    assert!(out.contains("Today          —"));
}

#[test]
fn overview_top_lines_rank_by_cost_not_tokens() {
    let l = Ledger::open_in_memory().unwrap();
    let now = runtab::timeutil::now_rfc3339();
    // model-cheap leads on tokens; model-pricey leads on cost.
    insert(&l, &ev("m1", "s1", "/home/u/projCheap", "model-cheap", &now, 10_000_000, Estimated, Some(1.0)));
    insert(&l, &ev("m2", "s2", "/home/u/projPricey", "model-pricey", &now, 1_000_000, Estimated, Some(5.0)));

    let out = runtab::overview::render(&l, &Style::new(false)).unwrap();
    assert!(out.contains("Top model      model-pricey · $5.00 (83%)"), "got: {out}");
    assert!(out.contains("Top project    projPricey · $5.00 (83%)"), "got: {out}");
}
