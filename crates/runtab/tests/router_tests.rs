mod common;

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{ev, ev_src, insert};
use runtab::cmdnorm;
use runtab::ledger::Ledger;
use runtab::model::CostBasis::Estimated;
use runtab::model::{RtkCommandRow, ToolResultSeen, ToolUseSeen};
use runtab::rtkimport::attribute;
use tower::ServiceExt;

fn bash_use(session: &str, tool_use_id: &str, project: &str, cmd: &str, ts: &str) -> ToolUseSeen {
    ToolUseSeen {
        source: "claude_code".to_string(),
        session_id: session.to_string(),
        tool_use_id: tool_use_id.to_string(),
        ts: ts.to_string(),
        project: project.to_string(),
        tool_name: "Bash".to_string(),
        est_args_tokens: 5,
        bash_head_hashes: Some(serde_json::to_string(&cmdnorm::chain_head_hashes(cmd)).unwrap()),
        bash_chain_hashes: Some(serde_json::to_string(&cmdnorm::chain_hashes(cmd)).unwrap()),
    }
}

fn bash_result(session: &str, tool_use_id: &str, ts: &str, est_result_tokens: i64) -> ToolResultSeen {
    ToolResultSeen {
        source: "claude_code".to_string(),
        session_id: session.to_string(),
        tool_use_id: tool_use_id.to_string(),
        ts: ts.to_string(),
        est_result_tokens,
        is_error: false,
    }
}

fn rtk_row(rtk_row_id: i64, ts: &str, project: &str, cmd: &str, saved: i64) -> RtkCommandRow {
    RtkCommandRow {
        rtk_row_id,
        ts: ts.to_string(),
        project_path: project.to_string(),
        head_hash: cmdnorm::hash(&cmdnorm::head(cmd)),
        cmd_hash: cmdnorm::hash(cmd),
        raw_tokens: saved + 20,
        filtered_tokens: 20,
        saved_tokens: saved,
        exec_time_ms: 5,
    }
}

async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

fn seeded_app() -> axum::Router {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev("a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100, Estimated, None));
    insert(&l, &ev("b", "s2", "B", "m1", "2026-07-02T11:00:00Z", 200, Estimated, None));
    runtab::serve::app(Arc::new(Mutex::new(l)))
}

#[tokio::test]
async fn every_endpoint_returns_200() {
    let app = seeded_app();
    for uri in [
        "/api/summary",
        "/api/daily",
        "/api/models",
        "/api/projects",
        "/api/agents",
        "/api/sessions",
        "/api/tools",
        "/api/heatmap",
        "/api/planwindow",
        "/api/sync/status",
        "/api/settings",
    ] {
        let (status, _) = get_json(app.clone(), uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn summary_and_lists_have_contract_fields() {
    let app = seeded_app();

    let (_, summary) = get_json(app.clone(), "/api/summary").await;
    assert_eq!(summary["total_tokens"], 300);
    assert_eq!(summary["billing_mode"], "subscription");
    assert!(summary["modes"].is_array());

    let (_, models) = get_json(app.clone(), "/api/models").await;
    assert!(models["models"].is_array());

    let (_, sessions) = get_json(app.clone(), "/api/sessions?page=1&page_size=1").await;
    assert_eq!(sessions["total"], 2);
    assert_eq!(sessions["page_size"], 1);
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);

    let (_, status) = get_json(app.clone(), "/api/sync/status").await;
    assert_eq!(status["state"], "off");
    assert_eq!(status["enabled"], false);
}

#[tokio::test]
async fn settings_round_trip_over_http() {
    let app = seeded_app();
    let body = serde_json::json!({
        "billing_mode_override": "api",
        "plan_limit_5h_tokens": 123,
        "plan_limit_weekly_tokens": 456,
        "machine_name": "box"
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let (_, got) = get_json(app.clone(), "/api/settings").await;
    assert_eq!(got["billing_mode_override"], "api");
    assert_eq!(got["plan_limit_5h_tokens"], 123);
    assert_eq!(got["machine_name"], "box");
}

#[tokio::test]
async fn tools_endpoint_is_empty_shape_on_a_fresh_ledger() {
    let l = Ledger::open_in_memory().unwrap();
    let app = runtab::serve::app(Arc::new(Mutex::new(l)));

    let (status, body) = get_json(app, "/api/tools").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tools"].as_array().unwrap().len(), 0);
    assert!(body["rtk"].is_null());
}

#[tokio::test]
async fn tools_endpoint_reports_exact_calls_and_rtk_totals() {
    let l = Ledger::open_in_memory().unwrap();
    l.insert_pending_tool_use(&bash_use("s1", "toolu_1", "/home/u/p", "git status", "2026-07-01T10:00:00Z"))
        .unwrap();
    assert!(l.resolve_tool_result(&bash_result("s1", "toolu_1", "2026-07-01T10:00:01Z", 20)).unwrap());
    assert!(l.insert_rtk_event(&rtk_row(1, "2026-07-01T10:00:01.500000000+00:00", "/home/u/p", "git status", 400)).unwrap());
    assert_eq!(attribute(&l).unwrap().text, 1);

    let app = runtab::serve::app(Arc::new(Mutex::new(l)));
    let (status, body) = get_json(app, "/api/tools").await;
    assert_eq!(status, StatusCode::OK);

    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["tool_name"], "Bash");
    assert_eq!(tools[0]["calls"], 1);
    assert_eq!(tools[0]["est_args_tokens"], 5);
    assert_eq!(tools[0]["est_result_tokens"], 20);
    assert_eq!(tools[0]["est_total_tokens"], 25);

    assert_eq!(body["rtk"]["commands"], 1);
    assert_eq!(body["rtk"]["saved_tokens"], 400);
}

#[tokio::test]
async fn tools_endpoint_days_filter_excludes_older_rows() {
    let l = Ledger::open_in_memory().unwrap();
    let recent_ts = runtab::timeutil::epoch_to_rfc3339(runtab::timeutil::now_epoch() - 3600);

    l.insert_pending_tool_use(&bash_use("s1", "old", "/home/u/p", "git status", "2000-01-01T10:00:00Z"))
        .unwrap();
    assert!(l.resolve_tool_result(&bash_result("s1", "old", "2000-01-01T10:00:00Z", 10)).unwrap());
    l.insert_pending_tool_use(&bash_use("s2", "recent", "/home/u/p", "ls", &recent_ts)).unwrap();
    assert!(l.resolve_tool_result(&bash_result("s2", "recent", &recent_ts, 20)).unwrap());

    assert!(l.insert_rtk_event(&rtk_row(1, "2000-01-01T10:00:00.000000000+00:00", "/home/u/p", "git status", 80)).unwrap());
    assert!(l.insert_rtk_event(&rtk_row(2, &recent_ts, "/home/u/p", "ls", 40)).unwrap());
    assert_eq!(attribute(&l).unwrap().text, 2);

    let app = runtab::serve::app(Arc::new(Mutex::new(l)));
    let (_, body) = get_json(app, "/api/tools?days=7").await;
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["calls"], 1);
    assert_eq!(body["rtk"]["commands"], 1);
    assert_eq!(body["rtk"]["saved_tokens"], 40);
}

#[tokio::test]
async fn tools_endpoint_session_filter_scopes_to_one_session() {
    let l = Ledger::open_in_memory().unwrap();
    l.insert_pending_tool_use(&bash_use("s1", "a", "/home/u/p", "git status", "2026-07-01T10:00:00Z"))
        .unwrap();
    assert!(l.resolve_tool_result(&bash_result("s1", "a", "2026-07-01T10:00:00Z", 10)).unwrap());
    l.insert_pending_tool_use(&bash_use("s2", "b", "/home/u/p", "ls", "2026-07-01T11:00:00Z"))
        .unwrap();
    assert!(l.resolve_tool_result(&bash_result("s2", "b", "2026-07-01T11:00:00Z", 20)).unwrap());

    assert!(l.insert_rtk_event(&rtk_row(1, "2026-07-01T10:00:00.000000000+00:00", "/home/u/p", "git status", 80)).unwrap());
    assert!(l.insert_rtk_event(&rtk_row(2, "2026-07-01T11:00:00.000000000+00:00", "/home/u/p", "ls", 40)).unwrap());
    assert_eq!(attribute(&l).unwrap().text, 2);

    let app = runtab::serve::app(Arc::new(Mutex::new(l)));
    let (_, body) = get_json(app, "/api/tools?session=s1").await;
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["calls"], 1);
    assert_eq!(tools[0]["est_result_tokens"], 10);
    assert_eq!(body["rtk"]["commands"], 1);
    assert_eq!(body["rtk"]["saved_tokens"], 80);
}

#[tokio::test]
async fn tools_endpoint_rejects_zero_days() {
    let app = seeded_app();
    let (status, body) = get_json(app, "/api/tools?days=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "bad_request");
}

#[tokio::test]
async fn tools_endpoint_rejects_non_numeric_days() {
    let app = seeded_app();
    let resp = app
        .oneshot(Request::builder().uri("/api/tools?days=nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn sessions_carry_rtk_saved_tokens_only_for_an_attributed_session() {
    let l = Ledger::open_in_memory().unwrap();
    // s1 has TWO usage_events rows: the correlated-subquery join must report
    // the attributed total once per session, not once per underlying row
    // (see `SavedJoin`'s doc comment on the equivalent CLI-report trap).
    insert(&l, &ev("a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100, Estimated, None));
    insert(&l, &ev("a2", "s1", "A", "m1", "2026-07-01T10:00:02Z", 50, Estimated, None));
    insert(&l, &ev("b", "s2", "A", "m1", "2026-07-01T11:00:00Z", 100, Estimated, None));

    l.insert_pending_tool_use(&bash_use("s1", "a", "A", "git status", "2026-07-01T10:00:05Z"))
        .unwrap();
    assert!(l.resolve_tool_result(&bash_result("s1", "a", "2026-07-01T10:00:06Z", 10)).unwrap());
    assert!(l.insert_rtk_event(&rtk_row(1, "2026-07-01T10:00:06.500000000+00:00", "A", "git status", 400)).unwrap());
    assert_eq!(attribute(&l).unwrap().text, 1);

    let app = runtab::serve::app(Arc::new(Mutex::new(l)));
    let (_, body) = get_json(app, "/api/sessions").await;
    let sessions = body["sessions"].as_array().unwrap();
    let s1 = sessions.iter().find(|s| s["session_id"] == "s1").unwrap();
    let s2 = sessions.iter().find(|s| s["session_id"] == "s2").unwrap();
    assert_eq!(s1["event_count"], 2);
    assert_eq!(s1["rtk_saved_tokens"], 400, "must not double-count across s1's two usage_events rows");
    assert!(s2["rtk_saved_tokens"].is_null());
}

fn multi_agent_app() -> axum::Router {
    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev_src("claude_code", "a", "s1", "A", "m1", "2026-07-01T10:00:00Z", 100));
    insert(&l, &ev_src("codex", "b", "s2", "B", "gpt", "2026-07-02T11:00:00Z", 500));
    runtab::serve::app(Arc::new(Mutex::new(l)))
}

#[tokio::test]
async fn agents_payload_has_contract_fields_and_ranking() {
    let (_, body) = get_json(multi_agent_app(), "/api/agents").await;
    let agents = body["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 2);
    // Ranked by total tokens DESC: codex (500) first, hyphen form on the wire.
    assert_eq!(agents[0]["agent"], "codex");
    assert_eq!(agents[0]["total_tokens"], 500);
    assert_eq!(agents[1]["agent"], "claude-code");
    assert_eq!(agents[1]["total_tokens"], 100);
    // Mirrors the ModelRow shape (agent replaces model).
    assert_eq!(agents[1]["input_tokens"], 100);
    assert!(agents[1]["cache_read_tokens"].is_u64());
    assert!(agents[1]["est_cost_microusd"].is_u64());
    assert!(agents[1]["unpriced_events"].is_u64());
    assert!(agents[1]["share"].is_number());
}

#[tokio::test]
async fn sessions_payload_carries_agent_field() {
    let (_, body) = get_json(multi_agent_app(), "/api/sessions").await;
    let sessions = body["sessions"].as_array().unwrap();
    let s1 = sessions.iter().find(|s| s["session_id"] == "s1").unwrap();
    assert_eq!(s1["agent"], "claude-code");
    let s2 = sessions.iter().find(|s| s["session_id"] == "s2").unwrap();
    assert_eq!(s2["agent"], "codex");
}

#[tokio::test]
async fn agent_query_param_filters_and_bogus_yields_empty() {
    let app = multi_agent_app();

    let (_, summary) = get_json(app.clone(), "/api/summary?agent=claude-code").await;
    assert_eq!(summary["total_tokens"], 100);

    let (_, models) = get_json(app.clone(), "/api/models?agent=codex").await;
    let models = models["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["model"], "gpt");

    let (_, agents) = get_json(app.clone(), "/api/agents?agent=codex").await;
    let agents = agents["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["agent"], "codex");

    // Unknown agent → empty arrays / zeroed totals (same as an unknown project).
    let (_, summary) = get_json(app.clone(), "/api/summary?agent=bogus").await;
    assert_eq!(summary["total_tokens"], 0);
    let (_, models) = get_json(app.clone(), "/api/models?agent=bogus").await;
    assert_eq!(models["models"].as_array().unwrap().len(), 0);
    let (_, agents) = get_json(app.clone(), "/api/agents?agent=bogus").await;
    assert_eq!(agents["agents"].as_array().unwrap().len(), 0);
    let (_, sessions) = get_json(app, "/api/sessions?agent=bogus").await;
    assert_eq!(sessions["total"], 0);
}

#[tokio::test]
async fn heatmap_and_daily_apply_the_agent_filter() {
    // Recent timestamps so both rows land inside the heatmap's 364-day window;
    // exercises the clause + trailing-date-param ordering with `agent` set.
    let l = Ledger::open_in_memory().unwrap();
    let now = runtab::timeutil::now_epoch();
    let recent = runtab::timeutil::epoch_to_rfc3339(now - 3600);
    let day = &recent[..10];
    insert(&l, &ev_src("claude_code", "a", "s1", "A", "m1", &recent, 100));
    insert(&l, &ev_src("codex", "b", "s2", "B", "gpt", &recent, 500));
    let app = runtab::serve::app(Arc::new(Mutex::new(l)));

    let (status, heatmap) = get_json(app.clone(), "/api/heatmap?agent=codex").await;
    assert_eq!(status, StatusCode::OK);
    let cells = heatmap["days"].as_array().unwrap();
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0]["date"], day);
    assert_eq!(cells[0]["total_tokens"], 500);
    assert_eq!(heatmap["max_tokens"], 500);

    let (_, daily) = get_json(app, "/api/daily?agent=codex").await;
    let days = daily["days"].as_array().unwrap();
    assert_eq!(days.len(), 1);
    assert_eq!(days[0]["total_tokens"], 500);
}

#[tokio::test]
async fn planwindow_ignores_agent_query_param() {
    let l = Ledger::open_in_memory().unwrap();
    let now = runtab::timeutil::now_epoch();
    let recent = runtab::timeutil::epoch_to_rfc3339(now - 3600);
    insert(&l, &ev_src("claude_code", "a", "s1", "A", "m1", &recent, 100));
    insert(&l, &ev_src("codex", "b", "s2", "B", "gpt", &recent, 999));
    let app = runtab::serve::app(Arc::new(Mutex::new(l)));

    // Filtering to codex must not zero the Claude-plan gauge.
    let (status, body) = get_json(app, "/api/planwindow?agent=codex").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["applicable"], true);
    assert_eq!(body["rolling_5h"]["tokens_used"], 100);
}
