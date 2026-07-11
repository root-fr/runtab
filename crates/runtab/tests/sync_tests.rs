mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use common::{ev, insert, review_all};
use runtab::ledger::{Filter, Ledger};
use runtab::model::CostBasis::Estimated;
use runtab::model::{CostBasis, UsageEvent};
use runtab::sync::client::{PollOutcome, SyncClient};
use runtab::sync::{pull_all, push_all};
use runtab::wire::{
    DeleteResult, DeviceStartOk, MachineInfo, MachinesResponse, PollOk, PullResponse, PulledRecord,
    PushResult, SyncedRecord,
};
use serde::Deserialize;

#[derive(Default)]
struct ServerState {
    events: HashMap<String, (i64, SyncedRecord)>,
    next_seq: i64,
    mode: Mode,
}

/// Throttle behavior for `post_events`, set by a test before it pushes.
#[derive(Default)]
enum Mode {
    #[default]
    Off,
    /// Return 429 `rate_limited` for the next N calls, then behave normally.
    FailNTimes(u32),
    /// Accept normally for the next N calls, then always return 429 `quota_daily`.
    QuotaAfterCalls(u32),
}

type Shared = Arc<Mutex<ServerState>>;

#[derive(Deserialize)]
struct InBatch {
    events: Vec<SyncedRecord>,
}

#[derive(Deserialize)]
struct PollReq {
    device_code: String,
}

#[derive(Deserialize)]
struct EventsQuery {
    since: Option<i64>,
    exclude_machine: Option<String>,
}

async fn spawn_server() -> (String, Shared) {
    let state: Shared = Arc::new(Mutex::new(ServerState::default()));
    let app = Router::new()
        .route("/healthz", get(|| async { Json(serde_json::json!({"status":"ok"})) }))
        .route("/v1/auth/device/start", post(device_start))
        .route("/v1/auth/poll", post(poll))
        .route("/v1/events", post(post_events).get(get_events))
        .route("/v1/machines", get(machines))
        .route("/v1/account", delete(delete_account))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

async fn device_start() -> Json<DeviceStartOk> {
    Json(DeviceStartOk {
        request: "uh_test".to_string(),
        display_code: "WDJB-MJHT".to_string(),
        verification_uri: "https://runtab.ai/authorize".to_string(),
        verification_uri_complete: "https://runtab.ai/authorize?request=uh_test".to_string(),
        interval_s: 3,
        expires_in_s: 900,
    })
}

async fn poll(Json(req): Json<PollReq>) -> axum::response::Response {
    use axum::response::IntoResponse;
    if req.device_code.contains("pending") {
        return (StatusCode::PRECONDITION_REQUIRED, Json(serde_json::json!({"error":"authorization_pending"}))).into_response();
    }
    if req.device_code.contains("expired") {
        return (StatusCode::GONE, Json(serde_json::json!({"error":"authorization_expired"}))).into_response();
    }
    if req.device_code.contains("machinelimit") {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error":"quota_machines"}))).into_response();
    }
    Json(PollOk { token: "rt_test".to_string(), user_id: "usr_1".to_string(), email: "a@b.c".to_string() })
        .into_response()
}

async fn post_events(State(st): State<Shared>, Json(batch): Json<InBatch>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let mut s = st.lock().unwrap();
    match &mut s.mode {
        Mode::FailNTimes(n) if *n > 0 => {
            *n -= 1;
            drop(s);
            return throttled("rate_limited");
        }
        Mode::QuotaAfterCalls(0) => {
            drop(s);
            return throttled("quota_daily");
        }
        Mode::QuotaAfterCalls(n) => *n -= 1,
        _ => {}
    }
    let mut accepted = 0u64;
    for rec in batch.events {
        if s.events.contains_key(&rec.event_id) {
            continue;
        }
        s.next_seq += 1;
        let seq = s.next_seq;
        s.events.insert(rec.event_id.clone(), (seq, rec));
        accepted += 1;
    }
    Json(PushResult { accepted, rejected: 0, rejections: vec![] }).into_response()
}

/// A 429 with a zero `Retry-After` (tests stay instant) and the given error code.
fn throttled(error: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(axum::http::header::RETRY_AFTER, "0")],
        Json(serde_json::json!({ "error": error })),
    )
        .into_response()
}

/// Mirrors the real server's page size (`runtab-server::events::PAGE_SIZE`)
/// so tests can exercise real multi-page pulls.
const PAGE_SIZE: usize = 500;

async fn get_events(State(st): State<Shared>, Query(q): Query<EventsQuery>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let mut s = st.lock().unwrap();
    if let Mode::FailNTimes(n) = &mut s.mode {
        if *n > 0 {
            *n -= 1;
            drop(s);
            return throttled("rate_limited");
        }
    }
    let since = q.since.unwrap_or(0);
    let exclude = q.exclude_machine.unwrap_or_default();
    let mut rows: Vec<(i64, SyncedRecord)> = s
        .events
        .values()
        .filter(|(seq, r)| *seq > since && r.machine_id != exclude)
        .cloned()
        .collect();
    rows.sort_by_key(|(seq, _)| *seq);
    rows.truncate(PAGE_SIZE);
    let has_more = rows.len() == PAGE_SIZE;
    let next_since = rows.last().map(|(seq, _)| *seq).unwrap_or(since);
    let events = rows
        .into_iter()
        .map(|(server_seq, record)| PulledRecord { server_seq, record })
        .collect();
    Json(PullResponse { events, next_since, has_more }).into_response()
}

async fn machines(State(st): State<Shared>) -> Json<MachinesResponse> {
    let s = st.lock().unwrap();
    let mut seen = HashMap::new();
    for (_, r) in s.events.values() {
        seen.entry(r.machine_id.clone()).or_insert(r.machine_name.clone());
    }
    let machines = seen
        .into_iter()
        .map(|(machine_id, machine_name)| MachineInfo {
            machine_id,
            machine_name,
            created_at: None,
            last_seen: None,
            revoked: false,
        })
        .collect();
    Json(MachinesResponse { machines })
}

async fn delete_account(State(st): State<Shared>) -> Json<DeleteResult> {
    let mut s = st.lock().unwrap();
    let removed = s.events.len() as u64;
    s.events.clear();
    Json(DeleteResult { deleted: true, events_removed: removed, machines_removed: 1 })
}

fn record(event_id: &str, machine: &str) -> SyncedRecord {
    SyncedRecord {
        event_id: event_id.to_string(),
        ts: "2026-07-01T10:00:00Z".to_string(),
        agent: "claude-code".to_string(),
        model: "m1".to_string(),
        project_label: "Y".to_string(),
        session_id: "s".to_string(),
        machine_id: machine.to_string(),
        machine_name: machine.to_string(),
        input_tokens: 100,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        reasoning_tokens: 0,
        est_cost_microusd: 0,
        cost_basis: "estimated".to_string(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_poll_maps_all_states() {
    let (base, _) = spawn_server().await;
    let client = SyncClient::new(&base).unwrap();
    assert!(client.healthz().await.unwrap());
    let start = client.device_start("laptop", "dc_x").await.unwrap();
    assert_eq!(start.display_code, "WDJB-MJHT");
    assert!(matches!(client.auth_poll("dc_pending").await.unwrap(), PollOutcome::Pending));
    assert!(matches!(client.auth_poll("dc_expired").await.unwrap(), PollOutcome::Expired));
    assert!(matches!(client.auth_poll("dc_machinelimit").await.unwrap(), PollOutcome::MachineLimit));
    match client.auth_poll("dc_ok").await.unwrap() {
        PollOutcome::Confirmed { token, email, .. } => {
            assert_eq!(token, "rt_test");
            assert_eq!(email, "a@b.c");
        }
        _ => panic!("expected confirmed"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_is_idempotent_on_event_id() {
    let (base, state) = spawn_server().await;
    let client = SyncClient::new(&base).unwrap();
    let recs = vec![record("e1", "A"), record("e2", "A")];
    assert_eq!(client.push_events("t", &recs).await.unwrap().accepted, 2);
    assert_eq!(client.push_events("t", &recs).await.unwrap().accepted, 0);
    assert_eq!(state.lock().unwrap().events.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_pull_merges_two_machines() {
    let (base, _) = spawn_server().await;
    let client = SyncClient::new(&base).unwrap();

    let a = Ledger::open_in_memory().unwrap();
    insert(&a, &ev("a1", "sA1", "Y", "m1", "2026-07-01T10:00:00Z", 100, Estimated, None));
    insert(&a, &ev("a2", "sA2", "Y", "m1", "2026-07-01T11:00:00Z", 150, Estimated, None));
    insert(&a, &ev("a3", "sA3", "X", "m1", "2026-07-01T12:00:00Z", 50, Estimated, None));
    review_all(&a);
    let ma = Mutex::new(a);
    assert_eq!(push_all(&ma, &client, "tokA").await.unwrap().pushed, 3);

    let b = Ledger::open_in_memory().unwrap();
    let b_id = b.machine_id().to_string();
    insert(&b, &ev("b1", "sB1", "Y", "m1", "2026-07-02T10:00:00Z", 200, Estimated, None));
    insert(&b, &ev("b2", "sB2", "Y", "m1", "2026-07-02T11:00:00Z", 250, Estimated, None));
    insert(&b, &ev("b3", "sB3", "Z", "m1", "2026-07-02T12:00:00Z", 75, Estimated, None));
    review_all(&b);
    let mb = Mutex::new(b);
    push_all(&mb, &client, "tokB").await.unwrap();

    let pulled = pull_all(&mb, &client, "tokB", &b_id).await.unwrap();
    assert_eq!(pulled.pulled, 3); // A's three rows, not B's own

    let b = mb.into_inner().unwrap();
    let summary = b.api_summary(&Filter::default()).unwrap();
    assert_eq!(summary.machine_count, 2);

    let projects = b.api_projects(&Filter::default()).unwrap();
    let labels: Vec<&str> = projects.iter().map(|p| p.project_label.as_str()).collect();
    assert!(labels.contains(&"X") && labels.contains(&"Y") && labels.contains(&"Z"));
    let y = projects.iter().find(|p| p.project_label == "Y").unwrap();
    assert_eq!(y.total_tokens, 700); // A(100+150) + B(200+250), overlapping project

    // Re-pull advances nothing new (cursor + event_id idempotency).
    assert_eq!(pull_all(&mb_reopen(b), &client, "tokB", &b_id).await.unwrap().pulled, 0);
}

fn mb_reopen(b: Ledger) -> Mutex<Ledger> {
    Mutex::new(b)
}

fn seed(l: &Ledger, count: usize) {
    for i in 0..count {
        insert(l, &ev(&format!("e{i}"), &format!("s{i}"), "Y", "m1", "2026-07-01T10:00:00Z", 10, Estimated, None));
    }
    review_all(l);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_all_retries_past_rate_limit_and_completes() {
    let (base, state) = spawn_server().await;
    let client = SyncClient::new(&base).unwrap();

    let l = Ledger::open_in_memory().unwrap();
    seed(&l, 2500); // three 1000-cap batches
    state.lock().unwrap().mode = Mode::FailNTimes(2);

    let m = Mutex::new(l);
    let out = push_all(&m, &client, "tok").await.unwrap();
    assert_eq!(out.pushed, 2500);
    assert_eq!(out.batches, 3); // only accepted attempts count
    assert!(!out.quota_reached);

    let l = m.into_inner().unwrap();
    assert_eq!(l.pending_push_count().unwrap(), 0); // cursor fully advanced
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_all_stops_cleanly_on_daily_quota() {
    let (base, state) = spawn_server().await;
    let client = SyncClient::new(&base).unwrap();

    let l = Ledger::open_in_memory().unwrap();
    seed(&l, 1500); // two batches: first accepted, second hits the quota
    state.lock().unwrap().mode = Mode::QuotaAfterCalls(1);

    let m = Mutex::new(l);
    let out = push_all(&m, &client, "tok").await.unwrap();
    assert!(out.quota_reached);
    assert_eq!(out.quota_retry_after, 0);
    assert_eq!(out.pushed, 1000); // only the first batch was accepted
    assert_eq!(out.batches, 1);

    let l = m.into_inner().unwrap();
    // Cursor advanced past exactly the accepted batch, so a later run resumes here.
    assert_eq!(l.pending_push_count().unwrap(), 500);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_all_retries_past_rate_limit_and_drains_all_pages() {
    let (base, state) = spawn_server().await;
    let client = SyncClient::new(&base).unwrap();

    // Seed enough remote events for three GET pages (500 + 500 + 200) from a
    // separate machine, pushed while the server is untouched by throttling.
    let a = Ledger::open_in_memory().unwrap();
    seed(&a, 1200);
    let ma = Mutex::new(a);
    let pushed = push_all(&ma, &client, "tokA").await.unwrap();
    assert_eq!(pushed.pushed, 1200);

    // Now throttle the first two GET calls (the first page's first two
    // attempts) before letting the rest through.
    state.lock().unwrap().mode = Mode::FailNTimes(2);

    let b = Ledger::open_in_memory().unwrap();
    let b_id = b.machine_id().to_string();
    let mb = Mutex::new(b);
    let out = pull_all(&mb, &client, "tokB", &b_id).await.unwrap();
    assert_eq!(out.pulled, 1200); // every page applied despite the rate limit

    let b = mb.into_inner().unwrap();
    assert_eq!(b.sync_state().unwrap().pull_cursor, 1200); // cursor lands past the last page
}

#[test]
fn review_gate_and_prefs_shape_the_push() {
    use runtab::ledger::ReviewItem;

    let l = Ledger::open_in_memory().unwrap();
    insert(&l, &ev("e1", "s1", "acme-payments", "m1", "2026-07-01T10:00:00Z", 100, Estimated, None));
    insert(&l, &ev("e2", "s2", "secret", "m1", "2026-07-01T11:00:00Z", 50, Estimated, None));

    // Consent gate: nothing is pushable before the review.
    assert_eq!(l.pending_batch(100).unwrap().records.len(), 0);
    assert!(!l.projects_reviewed().unwrap());

    // Rename one project, exclude the other.
    l.set_project_review(&[
        ReviewItem { name: "acme-payments".to_string(), label: Some("client-x".to_string()), excluded: false },
        ReviewItem { name: "secret".to_string(), label: None, excluded: true },
    ])
    .unwrap();

    let batch = l.pending_batch(100).unwrap();
    assert!(l.projects_reviewed().unwrap());
    assert_eq!(batch.records.len(), 1); // the excluded project never leaves
    assert_eq!(batch.records[0].project_label, "client-x"); // the rename is honoured
    assert_eq!(batch.scanned, 2); // cursor still advances past the excluded row
    assert_eq!(l.pending_push_count().unwrap(), 1); // excluded row is not counted
}

/// A cumulative event that can grow in place: same dedup key, more tokens.
fn cumulative_event(msg: &str, source: &str, input: i64) -> UsageEvent {
    UsageEvent {
        source: source.to_string(),
        message_id: msg.to_string(),
        request_id: String::new(),
        session_id: "sess".to_string(),
        ts: "2026-07-01T10:00:00Z".to_string(),
        model: "claude-opus-4-8".to_string(),
        input_tokens: input,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        cache_1h_tokens: 0,
        cache_5m_tokens: 0,
        reasoning_tokens: 0,
        project: "p".to_string(),
        agent_version: String::new(),
        cost_usd: None,
        cost_basis: Estimated,
    }
}

#[test]
fn replaced_higher_sets_dirty() {
    use runtab::ledger::UpsertResult;
    let l = Ledger::open_in_memory().unwrap();
    assert_eq!(l.upsert(&cumulative_event("m1", "hermes", 100)).unwrap(), UpsertResult::Inserted);
    assert_eq!(
        l.upsert(&cumulative_event("m1", "hermes", 300)).unwrap(),
        UpsertResult::ReplacedHigher
    );

    // The grown row is flagged dirty so the append-only push cursor re-selects it.
    let batch = { review_all(&l); l.pending_batch(100).unwrap() };
    assert_eq!(batch.dirty_ids.len(), 1);
}

#[test]
fn dirty_row_below_cursor_is_re_selected_and_cleared() {
    use runtab::ledger::UpsertResult;
    let l = Ledger::open_in_memory().unwrap();
    l.upsert(&cumulative_event("m1", "hermes", 100)).unwrap();
    review_all(&l);

    // Push the row: advance the cursor past it, then clear its dirty flag.
    let batch = l.pending_batch(100).unwrap();
    assert_eq!(batch.records.len(), 1);
    let pushed_id = batch.max_id;
    l.set_last_pushed_id(pushed_id).unwrap();
    l.clear_dirty(&batch.dirty_ids).unwrap();

    // Nothing pending now: the row is below the cursor and not dirty.
    let drained = l.pending_batch(100).unwrap();
    assert_eq!(drained.records.len(), 0);
    assert_eq!(drained.dirty_ids.len(), 0);

    // The row grows in place -> ReplacedHigher -> dirty -> re-selected below the cursor.
    assert_eq!(
        l.upsert(&cumulative_event("m1", "hermes", 500)).unwrap(),
        UpsertResult::ReplacedHigher
    );
    let regrown = l.pending_batch(100).unwrap();
    assert_eq!(regrown.records.len(), 1, "grown row must re-push below the cursor");
    assert_eq!(regrown.dirty_ids.len(), 1);
    assert_eq!(regrown.records[0].input_tokens, 500);

    // Clearing the dirty flag stops re-selection.
    l.clear_dirty(&regrown.dirty_ids).unwrap();
    assert_eq!(l.pending_batch(100).unwrap().records.len(), 0);
}

#[test]
fn excluded_dirty_row_is_reported_and_cleared() {
    use runtab::ledger::{ReviewItem, UpsertResult};
    let l = Ledger::open_in_memory().unwrap();
    l.upsert(&cumulative_event("m1", "hermes", 100)).unwrap();
    // Exclude the project: the row never pushes, but its dirty flag must still be
    // reported so the caller can clear it (else it re-scans every batch forever).
    l.set_project_review(&[ReviewItem {
        name: "p".to_string(),
        label: None,
        excluded: true,
    }])
    .unwrap();

    let batch = l.pending_batch(100).unwrap();
    l.set_last_pushed_id(batch.max_id).unwrap();
    l.clear_dirty(&batch.dirty_ids).unwrap();

    // Grow it: it is dirty again, reported in dirty_ids even though excluded.
    assert_eq!(
        l.upsert(&cumulative_event("m1", "hermes", 400)).unwrap(),
        UpsertResult::ReplacedHigher
    );
    let regrown = l.pending_batch(100).unwrap();
    assert_eq!(regrown.records.len(), 0, "excluded row never pushes");
    assert_eq!(regrown.dirty_ids.len(), 1, "but its dirty flag is still reported");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grown_row_is_re_pushed_after_first_push() {
    let (base, _) = spawn_server().await;
    let client = SyncClient::new(&base).unwrap();

    let l = Ledger::open_in_memory().unwrap();
    l.upsert(&cumulative_event("m1", "hermes", 100)).unwrap();
    review_all(&l);
    let event_id = l.pending_batch(1000).unwrap().records[0].event_id.clone();

    let m = Mutex::new(l);
    assert_eq!(push_all(&m, &client, "tok").await.unwrap().pushed, 1);

    // The row grows in place: same dedup key, higher totals.
    {
        let l = m.lock().unwrap();
        l.upsert(&cumulative_event("m1", "hermes", 999)).unwrap();
    }

    // The same event_id is pending again despite being below the push cursor.
    let batch = m.lock().unwrap().pending_batch(1000).unwrap();
    let ids: Vec<&str> = batch.records.iter().map(|r| r.event_id.as_str()).collect();
    assert!(ids.contains(&event_id.as_str()), "grown row must re-appear in the batch");
    assert_eq!(batch.records[0].input_tokens, 999);
}

#[test]
fn upsert_remote_keeps_higher_and_ignores_lower() {
    let dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("runtab_upsert_remote_{}_{nanos}.db", std::process::id()));

    let mut rec = record("re1", "other");
    rec.input_tokens = 100;
    {
        let l = Ledger::open(&path).unwrap();
        // First pull inserts.
        l.upsert_remote(&PulledRecord { server_seq: 1, record: rec.clone() }).unwrap();
        // A strictly-higher re-pull replaces (carrying the fresh server_seq).
        let mut higher = rec.clone();
        higher.input_tokens = 500;
        l.upsert_remote(&PulledRecord { server_seq: 7, record: higher }).unwrap();
        // A lower re-pull is ignored.
        let mut lower = rec.clone();
        lower.input_tokens = 50;
        l.upsert_remote(&PulledRecord { server_seq: 9, record: lower }).unwrap();

        let summary = l.api_summary(&Filter::default()).unwrap();
        assert_eq!(summary.total_tokens, 500, "keep-higher must retain the 500-token row");
    }

    // Exactly one remote row survived, carrying the higher pull's server_seq.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM remote_events WHERE event_id = 're1'", [], |r| r.get(0))
        .unwrap();
    let seq: i64 = conn
        .query_row("SELECT server_seq FROM remote_events WHERE event_id = 're1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(seq, 7);
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

/// A usage event with an explicit source and cost basis, for the plan-window pin.
#[allow(clippy::too_many_arguments)]
fn sourced_event(msg: &str, source: &str, ts: &str, input: i64, basis: CostBasis) -> UsageEvent {
    UsageEvent {
        source: source.to_string(),
        message_id: msg.to_string(),
        request_id: format!("r-{msg}"),
        session_id: format!("s-{msg}"),
        ts: ts.to_string(),
        model: "claude-opus-4-8".to_string(),
        input_tokens: input,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        cache_1h_tokens: 0,
        cache_5m_tokens: 0,
        reasoning_tokens: 0,
        project: "p".to_string(),
        agent_version: String::new(),
        cost_usd: None,
        cost_basis: basis,
    }
}

#[test]
fn plan_windows_count_only_claude_code_tokens() {
    let l = Ledger::open_in_memory().unwrap();
    // One minute ago, so the event sits inside the rolling 5h window at any
    // wall-clock time of day.
    let ts = runtab::timeutil::epoch_to_rfc3339(runtab::timeutil::now_epoch() - 60);
    // Claude subscription usage counts toward the plan.
    insert(&l, &sourced_event("c1", "claude_code", &ts, 100, Estimated));
    // Foreign-agent estimated tokens must NOT contaminate the Claude plan gauges.
    insert(&l, &sourced_event("x1", "codex", &ts, 1_000, Estimated));
    insert(&l, &sourced_event("x2", "opencode", &ts, 10_000, Estimated));
    insert(&l, &sourced_event("x3", "hermes", &ts, 100_000, Estimated));

    let windows = l.api_planwindow(&Filter::default()).unwrap();
    assert!(windows.applicable);
    let five_h = windows.rolling_5h.unwrap();
    assert_eq!(five_h.tokens_used, 100, "only the 100 claude_code tokens count");
    let weekly = windows.weekly.unwrap();
    assert_eq!(weekly.tokens_used, 100);
}

#[test]
fn scan_all_with_empty_db_adapters_matches_scan() {
    // With no file adapters and no DB adapters, `scan_all` is exactly `scan`:
    // it scans nothing and never touches source_cursors. Empty slices keep the
    // test off the developer's real transcripts and DBs.
    let l = Ledger::open_in_memory().unwrap();
    let pricing = runtab::pricing::Pricing::load().unwrap();
    let no_files: Vec<Box<dyn runtab::adapters::Adapter>> = Vec::new();
    let no_dbs: Vec<Box<dyn runtab::adapters::DbAdapter>> = Vec::new();

    let base = runtab::scan(&l, &no_files, &pricing);
    let all = runtab::scan_all(&l, &no_files, &no_dbs, &pricing);

    assert_eq!(base.files_scanned, all.files_scanned);
    assert_eq!(base.events_inserted, all.events_inserted);
    assert_eq!(all.files_scanned, 0);
    assert_eq!(all.events_inserted, 0);
    assert!(l.source_cursor("opencode").unwrap().is_none());
    assert!(l.source_cursor("hermes").unwrap().is_none());
}
