use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection};

use runtab::adapters::HermesAdapter;
use runtab::ledger::Ledger;
use runtab::pricing::Pricing;
use runtab::{scan_db_source_at, ScanSummary};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_path(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "runtab_hermes_{prefix}_{}_{nanos}_{unique}.db",
        std::process::id()
    ))
}

/// v16 `sessions` DDL (checkout schema): the full column set the adapter maps.
const HERMES_V16_DDL: &str = "
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    source TEXT,
    user_id TEXT,
    model TEXT,
    model_config TEXT,
    system_prompt TEXT,
    parent_session_id TEXT,
    started_at REAL,
    ended_at REAL,
    end_reason TEXT,
    message_count INTEGER,
    tool_call_count INTEGER,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_write_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    cwd TEXT,
    git_branch TEXT,
    git_repo_root TEXT,
    billing_provider TEXT,
    billing_base_url TEXT,
    billing_mode TEXT,
    estimated_cost_usd REAL,
    actual_cost_usd REAL,
    cost_status TEXT,
    cost_source TEXT,
    pricing_version TEXT,
    title TEXT,
    api_call_count INTEGER,
    handoff_state TEXT,
    handoff_platform TEXT,
    handoff_error TEXT,
    rewind_count INTEGER,
    archived INTEGER
);";

/// Pre-v5 era `sessions`: no billing/cost columns, no cwd/git/handoff/rewind/
/// archived. Only the pre-billing core survives; every row filters out as
/// zero-usage because no token columns exist.
const HERMES_V5ERA_DDL: &str = "
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    source TEXT,
    user_id TEXT,
    model TEXT,
    parent_session_id TEXT,
    started_at REAL,
    ended_at REAL,
    end_reason TEXT,
    message_count INTEGER,
    tool_call_count INTEGER,
    title TEXT
);";

/// v20-shaped `sessions`: v16 columns plus the v18 gateway-consolidation extras
/// (unknown to the adapter, never read) and a `session_model_usage` sibling
/// table (unreleased, must be ignored — no double count).
const HERMES_V20_DDL: &str = "
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    source TEXT,
    user_id TEXT,
    model TEXT,
    model_config TEXT,
    system_prompt TEXT,
    parent_session_id TEXT,
    started_at REAL,
    ended_at REAL,
    end_reason TEXT,
    message_count INTEGER,
    tool_call_count INTEGER,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_write_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    cwd TEXT,
    git_branch TEXT,
    git_repo_root TEXT,
    billing_provider TEXT,
    billing_base_url TEXT,
    billing_mode TEXT,
    estimated_cost_usd REAL,
    actual_cost_usd REAL,
    cost_status TEXT,
    cost_source TEXT,
    pricing_version TEXT,
    title TEXT,
    api_call_count INTEGER,
    handoff_state TEXT,
    handoff_platform TEXT,
    handoff_error TEXT,
    rewind_count INTEGER,
    archived INTEGER,
    session_key TEXT,
    chat_id TEXT,
    chat_type TEXT,
    display_name TEXT,
    origin_json TEXT,
    expiry_finalized INTEGER,
    compression_failure_cooldown_until REAL,
    compression_failure_error TEXT
);
CREATE TABLE session_model_usage (
    session_id TEXT NOT NULL,
    model TEXT NOT NULL,
    billing_provider TEXT NOT NULL DEFAULT '',
    billing_base_url TEXT NOT NULL DEFAULT '',
    billing_mode TEXT NOT NULL DEFAULT '',
    api_call_count INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    estimated_cost_usd REAL NOT NULL DEFAULT 0,
    actual_cost_usd REAL NOT NULL DEFAULT 0,
    cost_status TEXT, cost_source TEXT,
    first_seen REAL, last_seen REAL,
    PRIMARY KEY (session_id, model, billing_provider, billing_base_url, billing_mode)
);";

#[derive(Clone)]
struct Row {
    id: &'static str,
    model: Option<&'static str>,
    parent: Option<&'static str>,
    started_at: TsVal,
    ended_at: TsVal,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    cwd: Option<&'static str>,
    git_repo_root: Option<&'static str>,
    billing_mode: Option<&'static str>,
    estimated_cost_usd: Option<f64>,
    actual_cost_usd: Option<f64>,
    cost_status: Option<&'static str>,
}

#[derive(Clone)]
enum TsVal {
    Real(f64),
    Int(i64),
    Text(&'static str),
    Null,
}

impl Default for Row {
    fn default() -> Row {
        Row {
            id: "sess-1",
            model: Some("anthropic/claude-sonnet-4.6"),
            parent: None,
            started_at: TsVal::Real(1_782_516_348.3495),
            ended_at: TsVal::Null,
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            cwd: None,
            git_repo_root: None,
            billing_mode: None,
            estimated_cost_usd: None,
            actual_cost_usd: None,
            cost_status: None,
        }
    }
}

fn bind_ts(conn: &Connection, sql: &str, id: &str, ts: &TsVal) {
    match ts {
        TsVal::Real(v) => conn.execute(sql, params![id, v]).unwrap(),
        TsVal::Int(v) => conn.execute(sql, params![id, v]).unwrap(),
        TsVal::Text(v) => conn.execute(sql, params![id, v]).unwrap(),
        TsVal::Null => conn.execute(sql, params![id, Option::<f64>::None]).unwrap(),
    };
}

fn insert_v16(conn: &Connection, r: &Row) {
    conn.execute(
        "INSERT INTO sessions
            (id, model, parent_session_id, input_tokens, output_tokens,
             cache_read_tokens, cache_write_tokens, reasoning_tokens,
             cwd, git_repo_root, billing_mode, estimated_cost_usd,
             actual_cost_usd, cost_status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            r.id, r.model, r.parent, r.input, r.output, r.cache_read, r.cache_write,
            r.reasoning, r.cwd, r.git_repo_root, r.billing_mode, r.estimated_cost_usd,
            r.actual_cost_usd, r.cost_status
        ],
    )
    .unwrap();
    // started_at / ended_at carry a runtime-chosen storage class, so they are
    // set with a typed second statement rather than the params! path above.
    bind_ts(
        conn,
        "UPDATE sessions SET started_at = ?2 WHERE id = ?1",
        r.id,
        &r.started_at,
    );
    bind_ts(
        conn,
        "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
        r.id,
        &r.ended_at,
    );
}

fn hermes_db(path: &Path, ddl: &str, rows: &[Row]) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(ddl).unwrap();
    for r in rows {
        insert_v16(&conn, r);
    }
}

/// A read connection on the ledger's own db file, so per-row token/cost/basis
/// details (which no public summary exposes) can be asserted directly.
struct LedgerReader {
    conn: Connection,
}

impl LedgerReader {
    fn open(path: &Path) -> LedgerReader {
        LedgerReader {
            conn: Connection::open(path).unwrap(),
        }
    }

    fn count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
            .unwrap()
    }

    fn row(&self, session_id: &str) -> HermesRow {
        self.conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens,
                        cache_creation_tokens, reasoning_tokens, model,
                        cost_usd, cost_basis, ts, project, session_id, source, dirty
                 FROM usage_events WHERE session_id = ?1",
                params![session_id],
                |r| {
                    Ok(HermesRow {
                        input: r.get(0)?,
                        output: r.get(1)?,
                        cache_read: r.get(2)?,
                        cache_creation: r.get(3)?,
                        reasoning: r.get(4)?,
                        model: r.get(5)?,
                        cost_usd: r.get(6)?,
                        cost_basis: r.get(7)?,
                        ts: r.get(8)?,
                        project: r.get(9)?,
                        session_id: r.get(10)?,
                        source: r.get(11)?,
                        dirty: r.get(12)?,
                    })
                },
            )
            .unwrap()
    }
}

#[allow(dead_code)]
struct HermesRow {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_creation: i64,
    reasoning: i64,
    model: String,
    cost_usd: Option<f64>,
    cost_basis: String,
    ts: String,
    project: String,
    session_id: String,
    source: String,
    dirty: i64,
}

fn scan(db_path: &Path, ledger_path: &Path) -> ScanSummary {
    let ledger = Ledger::open(ledger_path).unwrap();
    let pricing = Pricing::load().unwrap();
    let mut summary = ScanSummary::default();
    scan_db_source_at(&ledger, &HermesAdapter, &pricing, db_path, &mut summary);
    summary
}

#[test]
fn one_event_per_session_row_with_cumulative_counters() {
    let db = temp_path("one_event");
    let led = temp_path("one_event_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[Row {
            id: "s1",
            input: 100,
            output: 40,
            cache_read: 900,
            cache_write: 12,
            reasoning: 7,
            billing_mode: Some("subscription_included"),
            cost_status: Some("included"),
            cwd: Some("/home/u/proj"),
            ..Row::default()
        }],
    );

    let s = scan(&db, &led);
    assert_eq!(s.files_scanned, 1);
    assert_eq!(s.events_inserted, 1);
    assert_eq!(s.lines_skipped, 0);

    let r = LedgerReader::open(&led).row("s1");
    assert_eq!(r.source, "hermes");
    assert_eq!(r.input, 100);
    assert_eq!(r.output, 40);
    assert_eq!(r.cache_read, 900);
    assert_eq!(r.cache_creation, 12);
    assert_eq!(r.reasoning, 7);
    assert_eq!(r.model, "anthropic/claude-sonnet-4.6");
    assert_eq!(r.project, "/home/u/proj");
    // message_id == session_id, so exactly one row exists for this session.
    assert_eq!(LedgerReader::open(&led).count(), 1);
}

#[test]
fn cumulative_growth_replaces_higher_and_sets_dirty() {
    let db = temp_path("grow");
    let led = temp_path("grow_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[Row {
            id: "s1",
            input: 100,
            output: 40,
            ..Row::default()
        }],
    );
    let s1 = scan(&db, &led);
    assert_eq!(s1.events_inserted, 1);

    // Session grows in place: same id, larger cumulative counters.
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE sessions SET input_tokens = 250, output_tokens = 90 WHERE id = 's1'",
        [],
    )
    .unwrap();
    drop(conn);

    let s2 = scan(&db, &led);
    assert_eq!(s2.events_inserted, 0);
    assert_eq!(s2.duplicates_dropped, 1);

    let reader = LedgerReader::open(&led);
    assert_eq!(reader.count(), 1);
    let r = reader.row("s1");
    assert_eq!(r.input, 250);
    assert_eq!(r.output, 90);
    assert_eq!(r.dirty, 1);
}

#[test]
fn unchanged_row_is_kept_existing_and_not_marked_dirty() {
    let db = temp_path("unchanged");
    let led = temp_path("unchanged_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[Row {
            id: "s1",
            input: 100,
            output: 40,
            ..Row::default()
        }],
    );
    scan(&db, &led);
    let s2 = scan(&db, &led);
    assert_eq!(s2.events_inserted, 0);
    assert_eq!(s2.duplicates_dropped, 1);
    assert_eq!(LedgerReader::open(&led).row("s1").dirty, 0);
}

#[test]
fn upstream_delete_leaves_ledger_row_in_place() {
    let db = temp_path("del");
    let led = temp_path("del_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[Row {
            id: "s1",
            input: 100,
            output: 40,
            ..Row::default()
        }],
    );
    scan(&db, &led);

    let conn = Connection::open(&db).unwrap();
    conn.execute("DELETE FROM sessions WHERE id = 's1'", []).unwrap();
    drop(conn);

    let s2 = scan(&db, &led);
    assert_eq!(s2.events_inserted, 0);
    // Spend already happened: the row persists in the ledger.
    let reader = LedgerReader::open(&led);
    assert_eq!(reader.count(), 1);
    assert_eq!(reader.row("s1").input, 100);
    // row_count cursor reflects the now-empty upstream table.
    assert_eq!(reader
        .conn
        .query_row(
            "SELECT row_count FROM source_cursors WHERE source = 'hermes'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0);
}

#[test]
fn compression_chain_parent_and_child_are_two_events_summed() {
    let db = temp_path("chain");
    let led = temp_path("chain_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            Row {
                id: "parent",
                input: 1000,
                output: 200,
                cache_read: 5000,
                ..Row::default()
            },
            Row {
                id: "child",
                parent: Some("parent"),
                input: 300,
                output: 80,
                cache_read: 1500,
                ..Row::default()
            },
        ],
    );

    let s = scan(&db, &led);
    assert_eq!(s.events_inserted, 2);

    let reader = LedgerReader::open(&led);
    assert_eq!(reader.count(), 2);
    let total_input: i64 = reader
        .conn
        .query_row("SELECT SUM(input_tokens) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    let total_output: i64 = reader
        .conn
        .query_row("SELECT SUM(output_tokens) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    let total_cache: i64 = reader
        .conn
        .query_row("SELECT SUM(cache_read_tokens) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total_input, 1300);
    assert_eq!(total_output, 280);
    assert_eq!(total_cache, 6500);
}

// ---- cost matrix §6.5 (all six arms, top-down) ----

fn cost_row(id: &'static str, r: Row) -> Row {
    Row {
        id,
        input: 100,
        output: 40,
        ..r
    }
}

#[test]
fn cost_matrix_actual_cost_present_maps_logged() {
    let db = temp_path("cm_actual");
    let led = temp_path("cm_actual_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[cost_row(
            "s",
            Row {
                actual_cost_usd: Some(0.42),
                // Even with a subscription mode, a real dollar figure wins.
                billing_mode: Some("subscription_included"),
                cost_status: Some("included"),
                estimated_cost_usd: Some(0.10),
                ..Row::default()
            },
        )],
    );
    scan(&db, &led);
    let r = LedgerReader::open(&led).row("s");
    assert_eq!(r.cost_basis, "logged");
    assert_eq!(r.cost_usd, Some(0.42));
}

#[test]
fn cost_matrix_estimated_metered_route_maps_logged() {
    let db = temp_path("cm_metered");
    let led = temp_path("cm_metered_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            cost_row(
                "api",
                Row {
                    cost_status: Some("estimated"),
                    billing_mode: Some("official_models_api"),
                    estimated_cost_usd: Some(0.33),
                    ..Row::default()
                },
            ),
            cost_row(
                "docs",
                Row {
                    cost_status: Some("estimated"),
                    billing_mode: Some("official_docs_snapshot"),
                    estimated_cost_usd: Some(0.21),
                    ..Row::default()
                },
            ),
        ],
    );
    scan(&db, &led);
    let reader = LedgerReader::open(&led);
    let api = reader.row("api");
    assert_eq!(api.cost_basis, "logged");
    assert_eq!(api.cost_usd, Some(0.33));
    let docs = reader.row("docs");
    assert_eq!(docs.cost_basis, "logged");
    assert_eq!(docs.cost_usd, Some(0.21));
}

#[test]
fn cost_matrix_estimated_subscription_route_never_logged() {
    let db = temp_path("cm_sub");
    let led = temp_path("cm_sub_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[cost_row(
            "s",
            Row {
                cost_status: Some("estimated"),
                billing_mode: Some("subscription_included"),
                estimated_cost_usd: Some(0.50),
                ..Row::default()
            },
        )],
    );
    scan(&db, &led);
    let r = LedgerReader::open(&led).row("s");
    assert_eq!(r.cost_basis, "estimated");
    // Defensive belt: a subscription route drops the figure to None so pricing
    // may fill the honest API-equivalent — it must never look metered.
    assert_eq!(r.cost_usd, None);
}

#[test]
fn cost_matrix_estimated_unknown_billing_mode_keeps_figure_as_estimated() {
    let db = temp_path("cm_unknown");
    let led = temp_path("cm_unknown_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            cost_row(
                "u_unknown",
                Row {
                    cost_status: Some("estimated"),
                    billing_mode: Some("unknown"),
                    estimated_cost_usd: Some(0.17),
                    ..Row::default()
                },
            ),
            cost_row(
                "u_null",
                Row {
                    cost_status: Some("estimated"),
                    billing_mode: None,
                    estimated_cost_usd: Some(0.19),
                    ..Row::default()
                },
            ),
        ],
    );
    scan(&db, &led);
    let reader = LedgerReader::open(&led);
    let a = reader.row("u_unknown");
    assert_eq!(a.cost_basis, "estimated");
    assert_eq!(a.cost_usd, Some(0.17));
    let b = reader.row("u_null");
    assert_eq!(b.cost_basis, "estimated");
    assert_eq!(b.cost_usd, Some(0.19));
}

#[test]
fn cost_matrix_included_maps_none_estimated() {
    let db = temp_path("cm_incl");
    let led = temp_path("cm_incl_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[cost_row(
            "s",
            Row {
                cost_status: Some("included"),
                billing_mode: Some("subscription_included"),
                estimated_cost_usd: Some(0.0),
                model: Some("house/fugu-uncounted-xyz"),
                ..Row::default()
            },
        )],
    );
    scan(&db, &led);
    let r = LedgerReader::open(&led).row("s");
    assert_eq!(r.cost_basis, "estimated");
    // included → None; model is unpriced (house model) → stays None.
    assert_eq!(r.cost_usd, None);
}

#[test]
fn cost_matrix_fallthrough_unknown_status_maps_none_estimated() {
    let db = temp_path("cm_fall");
    let led = temp_path("cm_fall_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            cost_row(
                "unk_status",
                Row {
                    cost_status: Some("unknown"),
                    billing_mode: Some("unknown"),
                    estimated_cost_usd: Some(0.30),
                    model: Some("house/fugu-uncounted-xyz"),
                    ..Row::default()
                },
            ),
            cost_row(
                "null_status",
                Row {
                    cost_status: None,
                    billing_mode: None,
                    estimated_cost_usd: None,
                    model: Some("house/fugu-uncounted-xyz"),
                    ..Row::default()
                },
            ),
        ],
    );
    scan(&db, &led);
    let reader = LedgerReader::open(&led);
    let a = reader.row("unk_status");
    assert_eq!(a.cost_basis, "estimated");
    assert_eq!(a.cost_usd, None);
    let b = reader.row("null_status");
    assert_eq!(b.cost_basis, "estimated");
    assert_eq!(b.cost_usd, None);
}

// ---- schema variants ----

#[test]
fn v5_era_schema_scans_without_error_and_emits_nothing() {
    // No token columns → every row is zero-usage → filtered out, not skipped.
    let db = temp_path("v5");
    let led = temp_path("v5_led");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(HERMES_V5ERA_DDL).unwrap();
    conn.execute(
        "INSERT INTO sessions (id, model, started_at) VALUES ('s1', 'anthropic/x', ?1)",
        params![1_782_516_348.3495_f64],
    )
    .unwrap();
    drop(conn);

    let s = scan(&db, &led);
    assert_eq!(s.files_scanned, 1);
    assert_eq!(s.events_inserted, 0);
    assert_eq!(s.lines_skipped, 0);
    assert_eq!(s.db_errors, 0);
    assert_eq!(LedgerReader::open(&led).count(), 0);
}

#[test]
fn v20_shaped_schema_ignores_extra_columns_and_session_model_usage() {
    let db = temp_path("v20");
    let led = temp_path("v20_led");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(HERMES_V20_DDL).unwrap();
    // v16-subset insert works against the wider table; extra cols stay NULL.
    conn.execute(
        "INSERT INTO sessions
            (id, model, input_tokens, output_tokens, cache_read_tokens,
             started_at, billing_mode, cost_status, estimated_cost_usd,
             session_key, chat_id)
         VALUES ('s1', 'anthropic/claude-sonnet-4.6', 111, 22, 333,
                 ?1, 'official_models_api', 'estimated', 0.25, 'sk', 'ch')",
        params![1_782_516_348.3495_f64],
    )
    .unwrap();
    // A session_model_usage row that would double-count if we read it.
    conn.execute(
        "INSERT INTO session_model_usage (session_id, model, input_tokens, output_tokens)
         VALUES ('s1', 'anthropic/claude-sonnet-4.6', 111, 22)",
        [],
    )
    .unwrap();
    drop(conn);

    let s = scan(&db, &led);
    assert_eq!(s.events_inserted, 1);
    assert_eq!(s.db_errors, 0);
    let reader = LedgerReader::open(&led);
    assert_eq!(reader.count(), 1);
    let r = reader.row("s1");
    // Exactly the sessions-row counters, not doubled by session_model_usage.
    assert_eq!(r.input, 111);
    assert_eq!(r.output, 22);
    assert_eq!(r.cache_read, 333);
    assert_eq!(r.cost_basis, "logged");
    assert_eq!(r.cost_usd, Some(0.25));
}

// ---- model / project / ts normalization ----

#[test]
fn empty_model_becomes_unknown_with_tokens_kept() {
    let db = temp_path("model");
    let led = temp_path("model_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            cost_row("null_model", Row { model: None, ..Row::default() }),
            cost_row("empty_model", Row { model: Some(""), ..Row::default() }),
        ],
    );
    scan(&db, &led);
    let reader = LedgerReader::open(&led);
    assert_eq!(reader.row("null_model").model, "unknown");
    assert_eq!(reader.row("null_model").input, 100);
    assert_eq!(reader.row("empty_model").model, "unknown");
}

#[test]
fn model_string_kept_verbatim_with_slash() {
    let db = temp_path("verbatim");
    let led = temp_path("verbatim_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[cost_row(
            "s",
            Row {
                model: Some("openrouter/anthropic/claude-sonnet-4.6"),
                ..Row::default()
            },
        )],
    );
    scan(&db, &led);
    assert_eq!(
        LedgerReader::open(&led).row("s").model,
        "openrouter/anthropic/claude-sonnet-4.6"
    );
}

#[test]
fn project_prefers_cwd_then_git_repo_root_then_empty() {
    let db = temp_path("proj");
    let led = temp_path("proj_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            cost_row(
                "with_cwd",
                Row {
                    cwd: Some("/home/u/a"),
                    git_repo_root: Some("/home/u/repo"),
                    ..Row::default()
                },
            ),
            cost_row(
                "git_only",
                Row {
                    cwd: None,
                    git_repo_root: Some("/home/u/repo"),
                    ..Row::default()
                },
            ),
            cost_row(
                "neither",
                Row {
                    cwd: None,
                    git_repo_root: None,
                    ..Row::default()
                },
            ),
        ],
    );
    scan(&db, &led);
    let reader = LedgerReader::open(&led);
    assert_eq!(reader.row("with_cwd").project, "/home/u/a");
    assert_eq!(reader.row("git_only").project, "/home/u/repo");
    assert_eq!(reader.row("neither").project, "");
}

#[test]
fn garbage_or_null_started_at_skips_row_and_counts_it() {
    let db = temp_path("badts");
    let led = temp_path("badts_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            cost_row(
                "null_ts",
                Row {
                    started_at: TsVal::Null,
                    ended_at: TsVal::Null,
                    ..Row::default()
                },
            ),
            cost_row(
                "garbage_ts",
                Row {
                    started_at: TsVal::Text("not-a-timestamp"),
                    ended_at: TsVal::Null,
                    ..Row::default()
                },
            ),
        ],
    );
    let s = scan(&db, &led);
    // Both usage-shaped rows have unusable timestamps → skipped + counted.
    assert_eq!(s.events_inserted, 0);
    assert_eq!(s.lines_skipped, 2);
    assert_eq!(LedgerReader::open(&led).count(), 0);
}

#[test]
fn started_at_null_falls_back_to_ended_at() {
    let db = temp_path("fallback");
    let led = temp_path("fallback_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[cost_row(
            "s",
            Row {
                started_at: TsVal::Null,
                ended_at: TsVal::Real(1_782_506_012.64544),
                ..Row::default()
            },
        )],
    );
    let s = scan(&db, &led);
    assert_eq!(s.events_inserted, 1);
    assert_eq!(s.lines_skipped, 0);
    // 1782506012 = 2026-06-06T... ; assert the date prefix normalized correctly.
    assert_eq!(&LedgerReader::open(&led).row("s").ts[..4], "2026");
}

#[test]
fn real_epoch_and_text_iso_timestamps_both_normalize() {
    let db = temp_path("ts_forms");
    let led = temp_path("ts_forms_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            cost_row(
                "real_epoch",
                Row {
                    started_at: TsVal::Real(1_782_516_348.3495),
                    ..Row::default()
                },
            ),
            cost_row(
                "int_epoch",
                Row {
                    started_at: TsVal::Int(1_782_516_348),
                    ..Row::default()
                },
            ),
            cost_row(
                "ms_epoch",
                Row {
                    started_at: TsVal::Int(1_782_516_348_349),
                    ..Row::default()
                },
            ),
            cost_row(
                "text_space",
                Row {
                    started_at: TsVal::Text("2026-06-06 12:34:56"),
                    ..Row::default()
                },
            ),
            cost_row(
                "text_iso_z",
                Row {
                    started_at: TsVal::Text("2026-06-06T12:34:56Z"),
                    ..Row::default()
                },
            ),
        ],
    );
    let s = scan(&db, &led);
    assert_eq!(s.events_inserted, 5);
    assert_eq!(s.lines_skipped, 0);
    let reader = LedgerReader::open(&led);
    // All land as RFC 3339 with a trailing Z and a T separator.
    for id in ["real_epoch", "int_epoch", "ms_epoch", "text_space", "text_iso_z"] {
        let ts = reader.row(id).ts;
        assert!(ts.ends_with('Z'), "{id}: {ts}");
        assert_eq!(ts.as_bytes()[10], b'T', "{id}: {ts}");
    }
    // Both epoch encodings (seconds and >10^12 ms) land on the same instant.
    assert_eq!(&reader.row("int_epoch").ts, "2026-06-26T23:25:48Z");
    assert_eq!(&reader.row("ms_epoch").ts, "2026-06-26T23:25:48.349Z");
    // TEXT ISO forms (space and Z separators) normalize to canonical T…Z.
    assert_eq!(&reader.row("text_space").ts, "2026-06-06T12:34:56Z");
    assert_eq!(&reader.row("text_iso_z").ts, "2026-06-06T12:34:56Z");
}

#[test]
fn zero_token_ghost_rows_are_not_emitted() {
    let db = temp_path("ghost");
    let led = temp_path("ghost_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            Row {
                id: "ghost",
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                ..Row::default()
            },
            cost_row("real", Row::default()),
        ],
    );
    let s = scan(&db, &led);
    assert_eq!(s.events_inserted, 1);
    assert_eq!(s.lines_skipped, 0);
    let reader = LedgerReader::open(&led);
    assert_eq!(reader.count(), 1);
    assert_eq!(reader.row("real").input, 100);
}

#[test]
fn cursor_is_empty_and_row_count_persisted() {
    let db = temp_path("cursor");
    let led = temp_path("cursor_led");
    hermes_db(
        &db,
        HERMES_V16_DDL,
        &[
            cost_row("a", Row::default()),
            Row {
                id: "ghost",
                ..Row::default()
            },
        ],
    );
    scan(&db, &led);
    let reader = LedgerReader::open(&led);
    let (cursor, row_count): (String, i64) = reader
        .conn
        .query_row(
            "SELECT cursor, row_count FROM source_cursors WHERE source = 'hermes'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(cursor, "");
    // row_count counts every upstream row, including the zero-token ghost.
    assert_eq!(row_count, 2);
}

#[test]
fn missing_sessions_table_skips_the_source_without_failing_the_sweep() {
    let db = temp_path("notable");
    let led = temp_path("notable_led");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE other (x);").unwrap();
    drop(conn);

    let s = scan(&db, &led);
    // fetch returns Err → source skipped, no file counted, no db error.
    assert_eq!(s.files_scanned, 0);
    assert_eq!(s.events_inserted, 0);
    assert_eq!(s.db_errors, 0);
    assert_eq!(LedgerReader::open(&led).count(), 0);
}
