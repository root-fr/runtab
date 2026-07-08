use rusqlite::Connection;

/// The keep-higher-total dedup and the report aggregates must sum the same set
/// of token columns; sharing one fragment keeps the SQL sites in step. Valid
/// against both `usage_events` and the `merged_events` view.
pub const TOTAL_TOKENS_EXPR: &str =
    "input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens + reasoning_tokens";

const BASE: &str = "
CREATE TABLE IF NOT EXISTS usage_events (
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
    UNIQUE(source, message_id, request_id)
);

CREATE TABLE IF NOT EXISTS scanned_files (
    path        TEXT    PRIMARY KEY,
    size        INTEGER NOT NULL,
    mtime       INTEGER NOT NULL,
    byte_offset INTEGER NOT NULL
);
";

// Adds the machine dimension, the pulled-rows table, sync/settings state, and
// the merged view the /api layer reads. Runs once on both fresh and existing
// databases (fresh DBs also start at user_version 0), so an in-place upgrade of
// a v0 ledger is lossless — only additive columns and new tables. The three
// `usage_events` columns are added conditionally (see `migrate_v1`); the rest is
// `IF NOT EXISTS` so a torn/replayed migration can never fail on a duplicate.
const MIGRATE_V1_TABLES: &str = "
CREATE TABLE IF NOT EXISTS remote_events (
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

CREATE TABLE IF NOT EXISTS sync_state (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    machine_id     TEXT    NOT NULL,
    machine_name   TEXT    NOT NULL,
    enabled        INTEGER NOT NULL DEFAULT 0,
    account_email  TEXT,
    user_id        TEXT,
    server_url     TEXT,
    pull_cursor    INTEGER NOT NULL DEFAULT 0,
    last_pushed_id INTEGER NOT NULL DEFAULT 0,
    last_push_at   TEXT,
    last_pull_at   TEXT,
    degraded       INTEGER NOT NULL DEFAULT 0,
    message        TEXT
);

CREATE TABLE IF NOT EXISTS app_settings (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    billing_mode_override    TEXT,
    plan_limit_5h_tokens     INTEGER NOT NULL DEFAULT 8000000,
    plan_limit_weekly_tokens INTEGER NOT NULL DEFAULT 100000000
);

CREATE VIEW IF NOT EXISTS merged_events AS
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
";

// Adds the pre-sync consent surface: per-project push preferences (rename /
// exclude) and a first-push review gate. `IF NOT EXISTS` + a conditional column
// keep it replay-safe like v1.
const MIGRATE_V2_TABLES: &str = "
CREATE TABLE IF NOT EXISTS project_prefs (
    name         TEXT PRIMARY KEY,
    synced_label TEXT,
    excluded     INTEGER NOT NULL DEFAULT 0
);
";

// Adds the tool-call event ledger: `tool_events` (finished calls, parsed from
// transcripts) with its in-flight `pending_tool_calls` staging table for
// tool_use rows awaiting their tool_result; and the rtk savings mirror:
// `rtk_events` (rows imported from rtk's own SQLite db, attributed to a
// session where possible) with `rtk_scan_state` tracking the import cursor
// and the attribution watermark. All four are new tables — no ALTER on
// existing ones — so `IF NOT EXISTS` alone keeps a torn/replayed migration
// safe. `rtk_scan_state` itself gained `last_attributed_rtk_id` as an
// in-place DDL edit (v3 is still branch-only, unreleased) rather than a v4
// migration.
const MIGRATE_V3_TABLES: &str = "
CREATE TABLE IF NOT EXISTS tool_events (
    id                INTEGER PRIMARY KEY,
    source            TEXT    NOT NULL,
    session_id        TEXT    NOT NULL,
    tool_use_id       TEXT    NOT NULL,
    ts                TEXT    NOT NULL,
    project           TEXT    NOT NULL,
    tool_name         TEXT    NOT NULL,
    est_args_tokens   INTEGER NOT NULL,
    est_result_tokens INTEGER NOT NULL,
    is_error          INTEGER NOT NULL DEFAULT 0,
    bash_head_hashes  TEXT,
    bash_chain_hashes TEXT,
    UNIQUE(source, session_id, tool_use_id)
);
CREATE INDEX IF NOT EXISTS idx_tool_events_session ON tool_events(source, session_id);
CREATE INDEX IF NOT EXISTS idx_tool_events_bash_ts ON tool_events(tool_name, ts);

CREATE TABLE IF NOT EXISTS pending_tool_calls (
    source            TEXT NOT NULL,
    session_id        TEXT NOT NULL,
    tool_use_id       TEXT NOT NULL,
    ts                TEXT NOT NULL,
    project           TEXT NOT NULL,
    tool_name         TEXT NOT NULL,
    est_args_tokens   INTEGER NOT NULL,
    bash_head_hashes  TEXT,
    bash_chain_hashes TEXT,
    PRIMARY KEY (source, session_id, tool_use_id)
);

CREATE TABLE IF NOT EXISTS rtk_events (
    id              INTEGER PRIMARY KEY,
    rtk_row_id      INTEGER NOT NULL UNIQUE,
    ts              TEXT    NOT NULL,
    project_path    TEXT    NOT NULL,
    head_hash       TEXT    NOT NULL,
    cmd_hash        TEXT    NOT NULL,
    raw_tokens      INTEGER NOT NULL,
    filtered_tokens INTEGER NOT NULL,
    saved_tokens    INTEGER NOT NULL,
    exec_time_ms    INTEGER NOT NULL,
    source          TEXT,
    session_id      TEXT,
    tool_event_id   INTEGER,
    match_kind      TEXT    NOT NULL DEFAULT 'none'
);
CREATE INDEX IF NOT EXISTS idx_rtk_events_match ON rtk_events(match_kind, ts);
DROP INDEX IF EXISTS idx_rtk_events_session;
CREATE INDEX IF NOT EXISTS idx_rtk_events_session ON rtk_events(session_id, source);
CREATE INDEX IF NOT EXISTS idx_rtk_events_project ON rtk_events(project_path, ts);

CREATE TABLE IF NOT EXISTS rtk_scan_state (
    id                      INTEGER PRIMARY KEY CHECK (id = 1),
    db_path                 TEXT NOT NULL,
    last_row_id             INTEGER NOT NULL DEFAULT 0,
    last_attributed_rtk_id  INTEGER NOT NULL DEFAULT 0
);
";

pub fn create(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(BASE)?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        migrate_v1(conn)?;
    }
    if version < 2 {
        migrate_v2(conn)?;
    }
    if version < 3 {
        migrate_v3(conn)?;
    }
    Ok(())
}

/// Apply the v1 migration atomically. `user_version` and DDL are transactional in
/// SQLite, so a crash between the ALTERs and the version bump rolls the whole
/// step back rather than leaving `user_version = 0` with the columns half-added
/// (which would brick every later open on `duplicate column name`). The ALTERs
/// are also emitted only for missing columns so a torn pre-fix database still
/// heals on the next open.
fn migrate_v1(conn: &Connection) -> rusqlite::Result<()> {
    let mut ddl = String::from("BEGIN;\n");
    for (col, ty) in [
        ("project_label", "TEXT"),
        ("machine_id", "TEXT"),
        ("machine_name", "TEXT"),
    ] {
        if !column_exists(conn, "usage_events", col)? {
            ddl.push_str(&format!("ALTER TABLE usage_events ADD COLUMN {col} {ty};\n"));
        }
    }
    ddl.push_str(MIGRATE_V1_TABLES);
    ddl.push_str("PRAGMA user_version = 1;\nCOMMIT;\n");
    conn.execute_batch(&ddl)
}

fn migrate_v2(conn: &Connection) -> rusqlite::Result<()> {
    let mut ddl = String::from("BEGIN;\n");
    if !column_exists(conn, "sync_state", "projects_reviewed")? {
        ddl.push_str("ALTER TABLE sync_state ADD COLUMN projects_reviewed INTEGER NOT NULL DEFAULT 0;\n");
    }
    ddl.push_str(MIGRATE_V2_TABLES);
    ddl.push_str("PRAGMA user_version = 2;\nCOMMIT;\n");
    conn.execute_batch(&ddl)
}

fn migrate_v3(conn: &Connection) -> rusqlite::Result<()> {
    let mut ddl = String::from("BEGIN;\n");
    ddl.push_str(MIGRATE_V3_TABLES);
    ddl.push_str("PRAGMA user_version = 3;\nCOMMIT;\n");
    conn.execute_batch(&ddl)
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}
