use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection};

use runtab::adapters::{ClaudeCodeAdapter, HermesAdapter, OpencodeAdapter};
use runtab::ledger::{Filter, Ledger};
use runtab::pricing::Pricing;
use runtab::{scan_db_source_at, scan_file, ScanSummary};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

static BILLING_COUNTER: AtomicU64 = AtomicU64::new(0);

fn billing_temp(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let unique = BILLING_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "runtab_billing_{prefix}_{}_{nanos}_{unique}.db",
        std::process::id()
    ))
}

const HERMES_SESSIONS_DDL: &str = "
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    model TEXT,
    started_at REAL,
    ended_at REAL,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_write_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    cwd TEXT,
    git_repo_root TEXT,
    billing_mode TEXT,
    estimated_cost_usd REAL,
    actual_cost_usd REAL,
    cost_status TEXT
);";

#[allow(clippy::too_many_arguments)]
fn insert_hermes_session(
    conn: &Connection,
    id: &str,
    input: i64,
    output: i64,
    billing_mode: &str,
    estimated_cost_usd: f64,
    cost_status: &str,
) {
    conn.execute(
        "INSERT INTO sessions
            (id, model, started_at, input_tokens, output_tokens,
             billing_mode, estimated_cost_usd, cost_status)
         VALUES (?1, 'anthropic/claude-sonnet-4.6', ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            1_782_516_348.3495_f64,
            input,
            output,
            billing_mode,
            estimated_cost_usd,
            cost_status
        ],
    )
    .unwrap();
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

#[test]
fn hermes_metered_rows_only_set_api_mode() {
    let db = billing_temp("hermes_api");
    let led = billing_temp("hermes_api_led");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(HERMES_SESSIONS_DDL).unwrap();
        // A genuinely metered route: cost_status=estimated + official_models_api
        // → Logged → api_events > 0.
        insert_hermes_session(&conn, "s1", 1000, 400, "official_models_api", 0.42, "estimated");
    }

    let ledger = Ledger::open(&led).unwrap();
    let pricing = Pricing::load().unwrap();
    let mut summary = ScanSummary::default();
    scan_db_source_at(&ledger, &HermesAdapter, &pricing, &db, &mut summary);
    assert_eq!(summary.events_inserted, 1);

    let s = ledger.api_summary(&Filter::default()).unwrap();
    assert_eq!(s.billing_mode, "api");
    assert_eq!(s.cost_label, "estimated spend");
    assert!(!s.plan_window_applicable);
}

#[test]
fn hermes_subscription_and_metered_rows_set_mixed_mode() {
    let db = billing_temp("hermes_mixed");
    let led = billing_temp("hermes_mixed_led");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(HERMES_SESSIONS_DDL).unwrap();
        // subscription_included → Estimated → sub_events.
        insert_hermes_session(&conn, "sub", 500, 200, "subscription_included", 0.0, "included");
        // official_models_api metered → Logged → api_events.
        insert_hermes_session(&conn, "api", 900, 300, "official_models_api", 0.55, "estimated");
    }

    let ledger = Ledger::open(&led).unwrap();
    let pricing = Pricing::load().unwrap();
    let mut summary = ScanSummary::default();
    scan_db_source_at(&ledger, &HermesAdapter, &pricing, &db, &mut summary);
    assert_eq!(summary.events_inserted, 2);

    let s = ledger.api_summary(&Filter::default()).unwrap();
    assert_eq!(s.billing_mode, "mixed");
}

#[test]
fn hermes_only_subscription_rows_stay_subscription_mode() {
    let db = billing_temp("hermes_sub");
    let led = billing_temp("hermes_sub_led");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(HERMES_SESSIONS_DDL).unwrap();
        insert_hermes_session(&conn, "s1", 500, 200, "subscription_included", 0.0, "included");
        // unknown billing_mode keeps its figure but stays Estimated (sub bucket).
        insert_hermes_session(&conn, "s2", 300, 100, "unknown", 0.11, "estimated");
    }

    let ledger = Ledger::open(&led).unwrap();
    let pricing = Pricing::load().unwrap();
    let mut summary = ScanSummary::default();
    scan_db_source_at(&ledger, &HermesAdapter, &pricing, &db, &mut summary);
    assert_eq!(summary.events_inserted, 2);

    let s = ledger.api_summary(&Filter::default()).unwrap();
    assert_eq!(s.billing_mode, "subscription");
}

const OPENCODE_BILLING_DDL: &str = "
CREATE TABLE project (id text PRIMARY KEY, worktree text NOT NULL);
CREATE TABLE session (id text PRIMARY KEY, project_id text NOT NULL, version text NOT NULL);
CREATE TABLE message (
    id text PRIMARY KEY, session_id text NOT NULL,
    time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL
);";

fn insert_opencode_assistant(conn: &Connection, id: &str, tu: i64, cost: f64) {
    let data = serde_json::json!({
        "id": id,
        "role": "assistant",
        "sessionID": "ses_b",
        "modelID": "claude-sonnet-4-5",
        "tokens": { "input": 100, "output": 50, "reasoning": 0,
                    "cache": { "read": 200, "write": 30 } },
        "cost": cost,
        "time": { "created": tu, "completed": tu },
        "path": { "cwd": "/p", "root": "/p" }
    })
    .to_string();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
         VALUES (?1, 'ses_b', ?2, ?2, ?3)",
        params![id, tu, data],
    )
    .unwrap();
}

#[test]
fn opencode_cost_rows_stay_subscription_mode() {
    let db = billing_temp("opencode_sub");
    let led = billing_temp("opencode_sub_led");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(OPENCODE_BILLING_DDL).unwrap();
        conn.execute("INSERT INTO project VALUES ('prj_1', '/wt')", []).unwrap();
        conn.execute("INSERT INTO session VALUES ('ses_b', 'prj_1', '1.2.6')", [])
            .unwrap();
        // opencode's cost is a synthetic models.dev figure even under
        // subscription auth → always Estimated → never flips mode to api.
        insert_opencode_assistant(&conn, "msg_1", 1_771_000_000_000, 0.42);
        insert_opencode_assistant(&conn, "msg_2", 1_771_000_000_010, 0.10);
    }

    let ledger = Ledger::open(&led).unwrap();
    let pricing = Pricing::load().unwrap();
    let mut summary = ScanSummary::default();
    scan_db_source_at(&ledger, &OpencodeAdapter, &pricing, &db, &mut summary);
    assert_eq!(summary.events_inserted, 2);

    let s = ledger.api_summary(&Filter::default()).unwrap();
    assert_eq!(s.billing_mode, "subscription");
}
