mod common;

use common::{ev, ev_src, insert};
use runtab::ledger::{Filter, Ledger};
use runtab::model::CostBasis::{Estimated, Logged};
use runtab::timeutil::{date_minus_days, epoch_to_rfc3339, now_epoch, today_utc};
use runtab::wire::{PulledRecord, SyncedRecord};

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

fn agent(f: Filter, source: &str) -> Filter {
    Filter { agent: Some(source.to_string()), ..f }
}

#[test]
fn agents_rollup_ranks_and_emits_hyphen_form() {
    let l = Ledger::open_in_memory().unwrap();
    // claude_code: 100 + 200 = 300 over two events; codex: 500 over one.
    insert(&l, &ev_src("claude_code", "a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100));
    insert(&l, &ev_src("claude_code", "b", "s1", "A", "m1", "2026-07-01T11:00:00Z", 200));
    insert(&l, &ev_src("codex", "c", "s2", "B", "gpt", "2026-07-02T10:00:00Z", 500));

    let rows = l.api_agents(&all()).unwrap();
    assert_eq!(rows.len(), 2);
    // Ranked by total tokens DESC: codex (500) before claude-code (300).
    assert_eq!(rows[0].agent, "codex");
    assert_eq!(rows[0].total_tokens, 500);
    assert_eq!(rows[1].agent, "claude-code"); // hyphen form on the wire
    assert_eq!(rows[1].total_tokens, 300);
    assert_eq!(rows[1].input_tokens, 300);
    // Shares sum to 1 over the two agents.
    assert!((rows.iter().map(|r| r.share).sum::<f64>() - 1.0).abs() < 1e-9);
}

#[test]
fn agent_filter_narrows_summary_models_daily() {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev_src("claude_code", "a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100));
    insert(&l, &ev_src("codex", "c", "s2", "B", "gpt", "2026-07-02T10:00:00Z", 500));

    // The wire value is hyphenated; the folded underscore form is what Filter stores.
    let f = agent(Filter::default(), "codex");
    assert_eq!(l.api_summary(&f).unwrap().total_tokens, 500);
    let models = l.api_models(&f).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model, "gpt");
    let days = l.api_daily(&f).unwrap();
    assert_eq!(days.len(), 1);
    assert_eq!(days[0].date, "2026-07-02");
    assert_eq!(days[0].total_tokens, 500);

    // The claude_code slice is disjoint.
    let cc = agent(Filter::default(), "claude_code");
    assert_eq!(l.api_summary(&cc).unwrap().total_tokens, 100);
}

#[test]
fn unknown_agent_yields_empty() {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev_src("claude_code", "a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100));

    let f = agent(Filter::default(), "nope");
    assert_eq!(l.api_summary(&f).unwrap().total_tokens, 0);
    assert!(l.api_models(&f).unwrap().is_empty());
    assert!(l.api_daily(&f).unwrap().is_empty());
    assert_eq!(l.api_sessions(&f, 1, 50).unwrap().total, 0);
    assert!(l.api_agents(&f).unwrap().is_empty());
}

#[test]
fn sessions_carry_agent_in_hyphen_form() {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev_src("claude_code", "a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100));
    insert(&l, &ev_src("codex", "c", "s2", "B", "gpt", "2026-07-02T10:00:00Z", 500));

    let page = l.api_sessions(&all(), 1, 50).unwrap();
    let s1 = page.sessions.iter().find(|s| s.session_id == "s1").unwrap();
    assert_eq!(s1.agent, "claude-code");
    let s2 = page.sessions.iter().find(|s| s.session_id == "s2").unwrap();
    assert_eq!(s2.agent, "codex");
}

fn synced(event_id: &str, agent: &str, input: u64) -> PulledRecord {
    PulledRecord {
        server_seq: 1,
        record: SyncedRecord {
            event_id: event_id.to_string(),
            ts: "2026-07-01T10:00:00Z".to_string(),
            agent: agent.to_string(),
            model: "m1".to_string(),
            project_label: "Y".to_string(),
            session_id: format!("sess-{event_id}"),
            machine_id: "remote".to_string(),
            machine_name: "remote".to_string(),
            input_tokens: input,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
            est_cost_microusd: 0,
            cost_basis: "estimated".to_string(),
        },
    }
}

#[test]
fn synced_claude_code_row_matches_agent_filter_and_emits_hyphen_form() {
    let l = Ledger::open_in_memory().unwrap();
    // A pulled row carries the wire (hyphen) agent form; the merged view folds it
    // to the local underscore `source`, so the underscore-form filter matches it
    // and `/api/agents` emits it back hyphenated.
    l.upsert_remote(&synced("re1", "claude-code", 100)).unwrap();

    let f = agent(Filter::default(), "claude_code");
    assert_eq!(l.api_summary(&f).unwrap().total_tokens, 100);

    let rows = l.api_agents(&all()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].agent, "claude-code");
    assert_eq!(rows[0].total_tokens, 100);
}

#[test]
fn planwindow_ignores_the_agent_filter() {
    let l = Ledger::open_in_memory().unwrap();
    let now = now_epoch();
    // A claude_code event inside the 5h window; a codex event that must never
    // count against the Claude plan gauge.
    insert(&l, &ev_src("claude_code", "a", "s1", "A", "m1", &epoch_to_rfc3339(now - 3600), 100));
    insert(&l, &ev_src("codex", "b", "s2", "B", "gpt", &epoch_to_rfc3339(now - 3600), 999));

    // Filtering to codex must NOT zero the gauge: the agent param is stripped,
    // and the claude_code pin holds — the 5h window still reports the 100 tokens.
    let f = agent(Filter::default(), "codex");
    let pw = l.api_planwindow(&f).unwrap();
    assert!(pw.applicable);
    assert_eq!(pw.rolling_5h.unwrap().tokens_used, 100);
}
