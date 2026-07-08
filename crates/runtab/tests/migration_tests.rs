use std::path::PathBuf;

use runtab::ledger::{Filter, Ledger};

/// The v0 collector schema, before the machine dimension / sync tables existed.
const V0: &str = "
CREATE TABLE usage_events (
    id INTEGER PRIMARY KEY, source TEXT NOT NULL, message_id TEXT NOT NULL,
    request_id TEXT NOT NULL, session_id TEXT NOT NULL, ts TEXT NOT NULL,
    model TEXT NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL, cache_creation_tokens INTEGER NOT NULL,
    cache_1h_tokens INTEGER NOT NULL, cache_5m_tokens INTEGER NOT NULL,
    reasoning_tokens INTEGER NOT NULL, project TEXT NOT NULL, agent_version TEXT NOT NULL,
    cost_usd REAL, cost_basis TEXT NOT NULL,
    UNIQUE(source, message_id, request_id)
);
CREATE TABLE scanned_files (
    path TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime INTEGER NOT NULL,
    byte_offset INTEGER NOT NULL
);
INSERT INTO usage_events (source, message_id, request_id, session_id, ts, model,
    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
    cache_1h_tokens, cache_5m_tokens, reasoning_tokens, project, agent_version,
    cost_usd, cost_basis)
VALUES ('claude_code','m1','r1','s1','2026-07-01T10:00:00Z','claude-opus-4-8',
    100,50,10,0,0,0,0,'/home/u/projA','1.0.0',NULL,'estimated');
";

/// A post-v2 database: `usage_events` already carries the v1 machine columns,
/// plus the v1 sync tables and the v2 `project_prefs` table, with one row in
/// each to exercise the lossless-upgrade assertion.
const V2: &str = "
CREATE TABLE usage_events (
    id                    INTEGER PRIMARY KEY,
    source                TEXT    NOT NULL,
    message_id            TEXT    NOT NULL,
    request_id            TEXT    NOT NULL,
    session_id            TEXT    NOT NULL,
    ts                    TEXT    NOT NULL,
    model                 TEXT    NOT NULL,
    input_tokens          INTEGER NOT NULL,
    output_tokens         INTEGER NOT NULL,
    cache_read_tokens     INTEGER NOT NULL,
    cache_creation_tokens INTEGER NOT NULL,
    cache_1h_tokens       INTEGER NOT NULL,
    cache_5m_tokens       INTEGER NOT NULL,
    reasoning_tokens      INTEGER NOT NULL,
    project               TEXT    NOT NULL,
    agent_version         TEXT    NOT NULL,
    cost_usd              REAL,
    cost_basis            TEXT    NOT NULL,
    project_label         TEXT,
    machine_id            TEXT,
    machine_name          TEXT,
    UNIQUE(source, message_id, request_id)
);
CREATE TABLE scanned_files (
    path        TEXT    PRIMARY KEY,
    size        INTEGER NOT NULL,
    mtime       INTEGER NOT NULL,
    byte_offset INTEGER NOT NULL
);
CREATE TABLE remote_events (
    server_seq            INTEGER PRIMARY KEY,
    event_id              TEXT    NOT NULL UNIQUE,
    ts                    TEXT    NOT NULL,
    agent                 TEXT    NOT NULL,
    model                 TEXT    NOT NULL,
    project_label         TEXT    NOT NULL,
    session_id            TEXT    NOT NULL,
    machine_id            TEXT    NOT NULL,
    machine_name          TEXT    NOT NULL,
    input_tokens          INTEGER NOT NULL,
    output_tokens         INTEGER NOT NULL,
    cache_read_tokens     INTEGER NOT NULL,
    cache_creation_tokens INTEGER NOT NULL,
    reasoning_tokens      INTEGER NOT NULL,
    est_cost_microusd     INTEGER NOT NULL,
    cost_basis            TEXT    NOT NULL
);
CREATE TABLE sync_state (
    id                INTEGER PRIMARY KEY CHECK (id = 1),
    machine_id        TEXT    NOT NULL,
    machine_name      TEXT    NOT NULL,
    enabled           INTEGER NOT NULL DEFAULT 0,
    account_email     TEXT,
    user_id           TEXT,
    server_url        TEXT,
    pull_cursor       INTEGER NOT NULL DEFAULT 0,
    last_pushed_id    INTEGER NOT NULL DEFAULT 0,
    last_push_at      TEXT,
    last_pull_at      TEXT,
    degraded          INTEGER NOT NULL DEFAULT 0,
    message           TEXT,
    projects_reviewed INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE app_settings (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    billing_mode_override    TEXT,
    plan_limit_5h_tokens     INTEGER NOT NULL DEFAULT 8000000,
    plan_limit_weekly_tokens INTEGER NOT NULL DEFAULT 100000000
);
CREATE VIEW merged_events AS
    SELECT
        ts, model, project_label, session_id, machine_id, machine_name,
        input_tokens, output_tokens, cache_read_tokens,
        cache_creation_tokens, reasoning_tokens,
        CASE WHEN cost_usd IS NULL THEN NULL
             ELSE CAST(ROUND(cost_usd * 1000000) AS INTEGER) END AS est_cost_microusd,
        cost_basis,
        CASE WHEN cost_usd IS NULL THEN 1 ELSE 0 END AS unpriced
    FROM usage_events
    UNION ALL
    SELECT
        ts, model, project_label, session_id, machine_id, machine_name,
        input_tokens, output_tokens, cache_read_tokens,
        cache_creation_tokens, reasoning_tokens,
        est_cost_microusd, cost_basis, 0 AS unpriced
    FROM remote_events;
CREATE TABLE project_prefs (
    name         TEXT PRIMARY KEY,
    synced_label TEXT,
    excluded     INTEGER NOT NULL DEFAULT 0
);
INSERT INTO usage_events (source, message_id, request_id, session_id, ts, model,
    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
    cache_1h_tokens, cache_5m_tokens, reasoning_tokens, project, agent_version,
    cost_usd, cost_basis, project_label, machine_id, machine_name)
VALUES ('claude_code','m1','r1','s1','2026-07-01T10:00:00Z','claude-opus-4-8',
    100,50,10,0,0,0,0,'/home/u/projA','1.0.0',NULL,'estimated','projA','m-1','host-1');
INSERT INTO project_prefs (name, synced_label, excluded) VALUES ('projA', NULL, 1);
PRAGMA user_version = 2;
";

const TOOL_EVENT_TABLES: [&str; 4] =
    ["tool_events", "pending_tool_calls", "rtk_events", "rtk_scan_state"];

fn table_exists(conn: &rusqlite::Connection, table: &str) -> bool {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |r| r.get(0),
        )
        .unwrap();
    count == 1
}

fn temp_db() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("runtab_migr_{}_{nanos}.db", std::process::id()))
}

#[test]
fn v0_database_upgrades_in_place_losslessly() {
    let path = temp_db();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(V0).unwrap();
        // v0 databases carry user_version 0 (the default), which triggers the
        // additive v1 migration on next open.
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 0);
    }

    let ledger = Ledger::open(&path).unwrap();
    let summary = ledger.api_summary(&Filter::default()).unwrap();
    assert_eq!(summary.total_tokens, 160); // 100 + 50 + 10, preserved
    assert_eq!(summary.machine_count, 1); // backfilled to this machine

    // The full path was reduced to a basename label for the merged view.
    let projects = ledger.api_projects(&Filter::default()).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].project_label, "projA");

    // The migration is idempotent: re-opening does not double-apply.
    drop(ledger);
    let again = Ledger::open(&path).unwrap();
    assert_eq!(again.api_summary(&Filter::default()).unwrap().total_tokens, 160);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn torn_migration_recovers_instead_of_bricking() {
    // Reproduce a crash between the v1 ALTERs and the version bump: the columns
    // are present but user_version is still 0. A non-conditional migration would
    // fail forever on `duplicate column name`; the conditional one must heal.
    let path = temp_db();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(V0).unwrap();
        conn.execute_batch(
            "ALTER TABLE usage_events ADD COLUMN project_label TEXT;
             ALTER TABLE usage_events ADD COLUMN machine_id TEXT;
             ALTER TABLE usage_events ADD COLUMN machine_name TEXT;",
        )
        .unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 0, "torn state keeps user_version at 0");
    }

    let ledger = Ledger::open(&path).expect("torn v0 db must migrate, not brick");
    assert_eq!(ledger.api_summary(&Filter::default()).unwrap().total_tokens, 160);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn fresh_database_reaches_v3_with_tool_event_tables() {
    let path = temp_db();
    drop(Ledger::open(&path).unwrap());

    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 3);

    for table in TOOL_EVENT_TABLES {
        assert!(table_exists(&conn, table), "missing table {table}");
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn v2_database_upgrades_to_v3_losslessly() {
    let path = temp_db();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(V2).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 2);
    }

    let ledger = Ledger::open(&path).unwrap();
    let summary = ledger.api_summary(&Filter::default()).unwrap();
    assert_eq!(summary.total_tokens, 160); // v2 usage_events row, preserved

    // The v2 project_prefs row (the consent decision) survives the upgrade.
    let items = ledger.project_review_items().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "projA");
    assert!(items[0].excluded);
    drop(ledger);

    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 3);
    for table in TOOL_EVENT_TABLES {
        assert!(table_exists(&conn, table), "missing table {table}");
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn torn_v3_replay_does_not_error() {
    // `create()` runs on every `Ledger::open`; a second open on an
    // already-v3 database re-executes the same `IF NOT EXISTS` DDL and must
    // be a no-op, not a "table already exists" error.
    let path = temp_db();
    drop(Ledger::open(&path).unwrap());

    let ledger = Ledger::open(&path).expect("replaying the v3 migration must not error");
    assert_eq!(ledger.api_summary(&Filter::default()).unwrap().total_tokens, 0);
    drop(ledger);

    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 3);

    let _ = std::fs::remove_file(&path);
}
