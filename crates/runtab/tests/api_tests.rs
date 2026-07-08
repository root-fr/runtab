mod common;

use common::{ev, insert};
use runtab::ledger::{Filter, Ledger};
use runtab::model::CostBasis::{Estimated, Logged};
use runtab::timeutil::{date_minus_days, epoch_to_rfc3339, now_epoch, today_utc};

fn all() -> Filter {
    Filter::default()
}

#[test]
fn summary_on_empty_ledger_is_zeroed() {
    let l = Ledger::open_in_memory().unwrap();
    let s = l.api_summary(&all()).unwrap();
    assert_eq!(s.total_tokens, 0);
    assert_eq!(s.billing_mode, "subscription");
    assert_eq!(s.since, None);
    assert_eq!(s.machine_count, 0);
    assert!(l.api_daily(&all()).unwrap().is_empty());
    assert_eq!(l.api_sessions(&all(), 1, 50).unwrap().total, 0);
}

#[test]
fn summary_totals_and_cache_ratio() {
    let l = Ledger::open_in_memory().unwrap();
    let mut a = ev("a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100, Estimated, None);
    a.cache_read_tokens = 300; // total 400, cache-read ratio 0.75
    insert(&l, &a);
    insert(&l, &ev("b", "s2", "B", "m1", "2026-07-02T10:00:00Z", 200, Estimated, None));

    let s = l.api_summary(&all()).unwrap();
    assert_eq!(s.total_tokens, 600);
    assert_eq!(s.project_count, 2);
    assert_eq!(s.machine_count, 1);
    assert!((s.cache_read_ratio - 0.5).abs() < 1e-9); // 300 / 600
    assert_eq!(s.since.as_deref(), Some("2026-07-01"));
}

#[test]
fn billing_mode_auto_detects_and_override_wins() {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev("a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100, Estimated, None));
    insert(&l, &ev("b", "s1", "A", "m1", "2026-07-01T11:00:00Z", 100, Estimated, None));

    // Pure estimated → subscription.
    let s = l.api_summary(&all()).unwrap();
    assert_eq!(s.billing_mode, "subscription");
    assert!(s.plan_window_applicable);
    assert_eq!(s.modes.len(), 1);

    // Add a logged-cost event → mixed with a per-mode breakdown.
    insert(&l, &ev("c", "s2", "A", "m1", "2026-07-02T10:00:00Z", 100, Logged, Some(0.5)));
    let s = l.api_summary(&all()).unwrap();
    assert_eq!(s.billing_mode, "mixed");
    assert_eq!(s.modes.len(), 2);
    assert!(s.plan_window_applicable);

    // Override to api → single mode, gauge hidden.
    let mut settings = l.settings().unwrap();
    settings.billing_mode_override = Some("api".to_string());
    l.update_settings(&settings).unwrap();
    let s = l.api_summary(&all()).unwrap();
    assert_eq!(s.billing_mode, "api");
    assert!(!s.plan_window_applicable);
    assert_eq!(s.cost_label, "estimated spend");
    assert!(!l.api_planwindow(&all()).unwrap().applicable);
}

#[test]
fn planwindow_math_counts_rolling_and_weekly() {
    let l = Ledger::open_in_memory().unwrap();
    let now = now_epoch();
    insert(&l, &ev("a", "s1", "A", "m1", &epoch_to_rfc3339(now - 3600), 100, Estimated, None));
    insert(&l, &ev("b", "s1", "A", "m1", &epoch_to_rfc3339(now - 6 * 3600), 200, Estimated, None));
    insert(&l, &ev("c", "s1", "A", "m1", &epoch_to_rfc3339(now - 3 * 86400), 300, Estimated, None));
    insert(&l, &ev("d", "s1", "A", "m1", &epoch_to_rfc3339(now - 10 * 86400), 400, Estimated, None));

    let pw = l.api_planwindow(&all()).unwrap();
    assert!(pw.applicable);
    let r5 = pw.rolling_5h.unwrap();
    assert_eq!(r5.window_hours, 5);
    assert_eq!(r5.tokens_used, 100); // only the 1h-ago event
    assert_eq!(r5.limit_tokens, 8_000_000);
    let wk = pw.weekly.unwrap();
    assert_eq!(wk.tokens_used, 600); // 1h + 6h + 3d, not the 10d one
    assert!(pw.limits_are_estimates);
}

#[test]
fn daily_models_projects_sessions_shapes() {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev("a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100, Estimated, None));
    insert(&l, &ev("b", "s1", "A", "m2", "2026-07-01T11:00:00Z", 200, Estimated, None));
    insert(&l, &ev("c", "s2", "B", "m1", "2026-07-02T10:00:00Z", 300, Estimated, None));

    let days = l.api_daily(&all()).unwrap();
    assert_eq!(days.len(), 2);
    assert_eq!(days[0].date, "2026-07-01");
    assert_eq!(days[0].total_tokens, 300);
    assert_eq!(days[0].by_model.len(), 2);

    let models = l.api_models(&all()).unwrap();
    let m1 = models.iter().find(|m| m.model == "m1").unwrap();
    assert_eq!(m1.total_tokens, 400); // a + c
    assert!((models.iter().map(|m| m.share).sum::<f64>() - 1.0).abs() < 1e-9);

    let projects = l.api_projects(&all()).unwrap();
    let a = projects.iter().find(|p| p.project_label == "A").unwrap();
    assert_eq!(a.total_tokens, 300);
    assert_eq!(a.session_count, 1);

    let page = l.api_sessions(&all(), 1, 50).unwrap();
    assert_eq!(page.total, 2);
    let s1 = page.sessions.iter().find(|s| s.session_id == "s1").unwrap();
    assert_eq!(s1.model, "mixed"); // two models in the session
    assert_eq!(s1.event_count, 2);
    let s2 = page.sessions.iter().find(|s| s.session_id == "s2").unwrap();
    assert_eq!(s2.model, "m1");
}

#[test]
fn heatmap_marks_deletion_horizon() {
    let l = Ledger::open_in_memory().unwrap();
    let now = now_epoch();
    insert(&l, &ev("a", "s1", "A", "m1", &epoch_to_rfc3339(now), 500, Estimated, None));

    let (cells, max, horizon) = l.api_heatmap(&all()).unwrap();
    assert!(cells.iter().any(|c| c.date == today_utc()));
    assert_eq!(max, 500);
    assert_eq!(horizon, date_minus_days(&today_utc(), 30));
}

#[test]
fn project_filter_scopes_the_selection() {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev("a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100, Estimated, None));
    insert(&l, &ev("b", "s2", "B", "m1", "2026-07-01T11:00:00Z", 200, Estimated, None));

    let f = Filter { project: Some("A".to_string()), ..Filter::default() };
    let s = l.api_summary(&f).unwrap();
    assert_eq!(s.total_tokens, 100);
    assert_eq!(s.project_count, 1);
}
