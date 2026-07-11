use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection};
use serde_json::json;

use runtab::adapters::OpencodeAdapter;
use runtab::ledger::{Filter, Ledger};
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
        "runtab_opencode_{prefix}_{}_{nanos}_{unique}.db",
        std::process::id()
    ))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// v1.2.6 schema, verified against the live `~/.local/share/opencode/opencode.db`
/// via `sqlite3 -readonly '.schema'`. Only the columns the adapter reads plus
/// the FK structure that makes the LEFT JOINs meaningful.
const OPENCODE_DDL: &str = "
CREATE TABLE project (
    id text PRIMARY KEY,
    worktree text NOT NULL,
    vcs text,
    name text,
    icon_url text,
    icon_color text,
    time_created integer NOT NULL,
    time_updated integer NOT NULL,
    time_initialized integer,
    sandboxes text NOT NULL,
    commands text
);
CREATE TABLE session (
    id text PRIMARY KEY,
    project_id text NOT NULL,
    parent_id text,
    slug text NOT NULL,
    directory text NOT NULL,
    title text NOT NULL,
    version text NOT NULL,
    share_url text,
    revert text,
    permission text,
    time_created integer NOT NULL,
    time_updated integer NOT NULL,
    time_compacting integer,
    time_archived integer
);
CREATE TABLE message (
    id text PRIMARY KEY,
    session_id text NOT NULL,
    time_created integer NOT NULL,
    time_updated integer NOT NULL,
    data text NOT NULL
);
CREATE INDEX message_session_idx ON message (session_id);
";

/// Schema-drift variant: `project` table absent, so the rich JOIN query fails to
/// prepare and the adapter falls back to the core-columns query.
const OPENCODE_NO_PROJECT_DDL: &str = "
CREATE TABLE session (
    id text PRIMARY KEY,
    project_id text NOT NULL,
    version text NOT NULL,
    time_created integer NOT NULL
);
CREATE TABLE message (
    id text PRIMARY KEY,
    session_id text NOT NULL,
    time_created integer NOT NULL,
    time_updated integer NOT NULL,
    data text NOT NULL
);
";

struct OcDb {
    path: PathBuf,
    conn: Connection,
}

impl OcDb {
    fn new(prefix: &str) -> OcDb {
        OcDb::with_ddl(prefix, OPENCODE_DDL)
    }

    fn with_ddl(prefix: &str, ddl: &str) -> OcDb {
        let path = temp_path(prefix);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(ddl).unwrap();
        OcDb { path, conn }
    }

    fn project(&self, id: &str, worktree: &str) {
        self.conn
            .execute(
                "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes)
                 VALUES (?1, ?2, 0, 0, '[]')",
                params![id, worktree],
            )
            .unwrap();
    }

    fn session(&self, id: &str, project_id: &str, version: &str) {
        self.conn
            .execute(
                "INSERT INTO session
                    (id, project_id, slug, directory, title, version, time_created, time_updated)
                 VALUES (?1, ?2, 'slug', '/dir', 'title', ?3, 0, 0)",
                params![id, project_id, version],
            )
            .unwrap();
    }

    /// Session row for the drift schema (fewer columns).
    fn session_bare(&self, id: &str, project_id: &str, version: &str) {
        self.conn
            .execute(
                "INSERT INTO session (id, project_id, version, time_created)
                 VALUES (?1, ?2, ?3, 0)",
                params![id, project_id, version],
            )
            .unwrap();
    }

    fn assistant(&self, m: &AsstMsg) {
        let mut tokens = json!({
            "input": m.input,
            "output": m.output,
            "reasoning": m.reasoning,
            "cache": { "read": m.cache_read, "write": m.cache_write },
        });
        tokens["total"] = json!(m.input + m.output + m.cache_read + m.cache_write + m.reasoning);
        let mut time = json!({ "created": m.time_created });
        if let Some(c) = m.completed {
            time["completed"] = json!(c);
        }
        let mut data = json!({
            "id": m.id,
            "role": "assistant",
            "sessionID": m.session_id,
            "modelID": m.model,
            "providerID": "prov",
            "tokens": tokens,
            "cost": m.cost,
            "time": time,
            "path": { "cwd": m.cwd, "root": m.root },
        });
        if m.summary {
            data["summary"] = json!(true);
        }
        self.upsert_raw(m.id, m.session_id, m.time_created, m.time_updated, &data.to_string());
    }

    fn user(&self, id: &str, session_id: &str, tu: i64) {
        let data = json!({ "id": id, "role": "user", "sessionID": session_id }).to_string();
        self.upsert_raw(id, session_id, tu, tu, &data);
    }

    fn upsert_raw(&self, id: &str, session_id: &str, tc: i64, tu: i64, data: &str) {
        self.conn
            .execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET time_updated = ?4, data = ?5",
                params![id, session_id, tc, tu, data],
            )
            .unwrap();
    }

    fn delete_message(&self, id: &str) {
        self.conn
            .execute("DELETE FROM message WHERE id = ?1", params![id])
            .unwrap();
    }
}

/// Assistant message builder with sane defaults; `completed` present by default
/// (a quiescent, finished row that passes the Branch-2 gate).
struct AsstMsg<'a> {
    id: &'a str,
    session_id: &'a str,
    model: &'a str,
    time_created: i64,
    time_updated: i64,
    completed: Option<i64>,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    cost: f64,
    cwd: &'a str,
    root: &'a str,
    summary: bool,
}

impl<'a> AsstMsg<'a> {
    fn new(id: &'a str, session_id: &'a str) -> AsstMsg<'a> {
        AsstMsg {
            id,
            session_id,
            model: "claude-sonnet-4-5",
            time_created: 1_771_000_000_000,
            time_updated: 1_771_000_000_000,
            completed: Some(1_771_000_000_000),
            input: 100,
            output: 50,
            cache_read: 200,
            cache_write: 30,
            reasoning: 10,
            cost: 0.0,
            cwd: "/home/u/proj",
            root: "/home/u/proj",
            summary: false,
        }
    }
}

/// A read connection on the ledger's own db file, so per-row token/cost/basis
/// details (which no public summary exposes) can be asserted directly. Mirrors
/// the pattern the hermes adapter tests use.
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
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE source = 'opencode'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn sums(&self) -> (i64, i64, i64, i64, i64) {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_creation_tokens),0),
                        COALESCE(SUM(reasoning_tokens),0)
                 FROM usage_events WHERE source = 'opencode'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap()
    }

    fn row(&self, message_id: &str) -> OcRow {
        self.conn
            .query_row(
                "SELECT source, message_id, request_id, session_id, ts, model, project,
                        agent_version, cost_usd, cost_basis, input_tokens, output_tokens,
                        cache_read_tokens, cache_creation_tokens, reasoning_tokens, dirty
                 FROM usage_events WHERE message_id = ?1",
                params![message_id],
                |r| {
                    Ok(OcRow {
                        source: r.get(0)?,
                        message_id: r.get(1)?,
                        request_id: r.get(2)?,
                        session_id: r.get(3)?,
                        ts: r.get(4)?,
                        model: r.get(5)?,
                        project: r.get(6)?,
                        agent_version: r.get(7)?,
                        cost_usd: r.get(8)?,
                        cost_basis: r.get(9)?,
                        input: r.get(10)?,
                        output: r.get(11)?,
                        cache_read: r.get(12)?,
                        cache_creation: r.get(13)?,
                        reasoning: r.get(14)?,
                        dirty: r.get(15)?,
                    })
                },
            )
            .unwrap()
    }
}

#[allow(dead_code)]
struct OcRow {
    source: String,
    message_id: String,
    request_id: String,
    session_id: String,
    ts: String,
    model: String,
    project: String,
    agent_version: String,
    cost_usd: Option<f64>,
    cost_basis: String,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_creation: i64,
    reasoning: i64,
    dirty: i64,
}

/// A scan against a fresh file-backed ledger reused across ticks by path.
struct Harness {
    ledger_path: PathBuf,
}

impl Harness {
    fn new(prefix: &str) -> Harness {
        Harness {
            ledger_path: temp_path(&format!("ledger_{prefix}")),
        }
    }

    fn scan(&self, db: &OcDb) -> ScanSummary {
        let ledger = Ledger::open(&self.ledger_path).unwrap();
        let pricing = Pricing::load().unwrap();
        let mut summary = ScanSummary::default();
        scan_db_source_at(&ledger, &OpencodeAdapter, &pricing, &db.path, &mut summary);
        summary
    }

    fn reader(&self) -> LedgerReader {
        LedgerReader::open(&self.ledger_path)
    }

    fn cursor(&self) -> Option<String> {
        let ledger = Ledger::open(&self.ledger_path).unwrap();
        ledger.source_cursor("opencode").unwrap().map(|s| s.cursor)
    }

    fn billing_mode(&self) -> String {
        let ledger = Ledger::open(&self.ledger_path).unwrap();
        ledger.api_summary(&Filter::default()).unwrap().billing_mode
    }
}

fn cursor_tu(cursor: &str) -> i64 {
    let v: serde_json::Value = serde_json::from_str(cursor).unwrap();
    v.get("tu").and_then(|x| x.as_i64()).unwrap()
}

// --- mapping exactness ---------------------------------------------------

#[test]
fn maps_every_field_from_the_assistant_row() {
    let db = OcDb::new("map");
    db.project("prj_1", "/wt/root");
    db.session("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg {
        model: "claude-sonnet-4-5",
        input: 100,
        output: 50,
        cache_read: 200,
        cache_write: 30,
        reasoning: 10,
        cost: 0.42,
        time_created: 1_771_465_371_335,
        cwd: "/home/u/proj",
        root: "/home/u/proj-root",
        ..AsstMsg::new("msg_1", "ses_a")
    });

    let h = Harness::new("map");
    h.scan(&db);
    let r = h.reader().row("msg_1");

    assert_eq!(r.source, "opencode");
    assert_eq!(r.message_id, "msg_1");
    assert_eq!(r.request_id, "");
    assert_eq!(r.session_id, "ses_a");
    // ts from time_created (creation time, stable across updates).
    assert_eq!(r.ts, "2026-02-19T01:42:51.335Z");
    // bare modelID so vendor prefixes hit pricing.
    assert_eq!(r.model, "claude-sonnet-4-5");
    // project fallback: cwd first.
    assert_eq!(r.project, "/home/u/proj");
    // version joined from session.
    assert_eq!(r.agent_version, "1.2.6");
    // cost > 0 kept, ALWAYS Estimated.
    assert_eq!(r.cost_usd, Some(0.42));
    assert_eq!(r.cost_basis, "estimated");
    assert_eq!((r.input, r.output, r.cache_read, r.cache_creation, r.reasoning), (100, 50, 200, 30, 10));
}

#[test]
fn cost_zero_maps_to_none_not_a_free_call() {
    let db = OcDb::new("costzero");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    // A priced model with cost 0: pricing.apply fills a snapshot estimate.
    db.assistant(&AsstMsg { cost: 0.0, model: "gpt-5", ..AsstMsg::new("msg_1", "ses_a") });

    let h = Harness::new("costzero");
    h.scan(&db);
    let r = h.reader().row("msg_1");
    // cost=0 -> None from the source -> snapshot fills it, still Estimated.
    assert!(r.cost_usd.is_some());
    assert_eq!(r.cost_basis, "estimated");
}

#[test]
fn project_falls_back_root_then_worktree() {
    let db = OcDb::new("projfb");
    db.project("prj_1", "/wt/joined");
    db.session("ses_a", "prj_1", "1.2.6");
    // no cwd -> root
    db.assistant(&AsstMsg { cwd: "", root: "/the/root", ..AsstMsg::new("m_root", "ses_a") });
    // no cwd, no root -> joined project.worktree
    db.assistant(&AsstMsg { cwd: "", root: "", ..AsstMsg::new("m_wt", "ses_a") });

    let h = Harness::new("projfb");
    h.scan(&db);
    let reader = h.reader();
    assert_eq!(reader.row("m_root").project, "/the/root");
    assert_eq!(reader.row("m_wt").project, "/wt/joined");
}

#[test]
fn user_rows_and_zero_token_rows_are_excluded() {
    let db = OcDb::new("filter");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    db.user("msg_user", "ses_a", 1_771_000_000_000);
    db.assistant(&AsstMsg {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reasoning: 0,
        ..AsstMsg::new("msg_zero", "ses_a")
    });
    db.assistant(&AsstMsg::new("msg_real", "ses_a"));

    let h = Harness::new("filter");
    h.scan(&db);
    let reader = h.reader();
    assert_eq!(reader.count(), 1);
    assert_eq!(reader.row("msg_real").message_id, "msg_real");
}

#[test]
fn summary_compaction_rows_are_included() {
    let db = OcDb::new("summary");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg::new("msg_normal", "ses_a"));
    db.assistant(&AsstMsg { summary: true, ..AsstMsg::new("msg_summary", "ses_a") });

    let h = Harness::new("summary");
    h.scan(&db);
    // both count — a compaction message is a real, separately billed LLM call.
    assert_eq!(h.reader().count(), 2);
}

#[test]
fn zero_token_row_appears_once_it_grows() {
    let db = OcDb::new("grow");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reasoning: 0,
        ..AsstMsg::new("msg_g", "ses_a")
    });

    let h = Harness::new("grow");
    h.scan(&db);
    assert_eq!(h.reader().count(), 0);

    db.assistant(&AsstMsg {
        input: 100,
        time_updated: 1_771_000_000_050,
        completed: Some(1_771_000_000_050),
        ..AsstMsg::new("msg_g", "ses_a")
    });
    h.scan(&db);
    assert_eq!(h.reader().count(), 1);
}

// --- streaming update / keep-higher --------------------------------------

#[test]
fn streaming_update_replaces_higher_and_marks_dirty() {
    let db = OcDb::new("stream");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg {
        output: 50,
        cache_read: 100,
        time_updated: 1_771_000_000_000,
        completed: Some(1_771_000_000_000),
        ..AsstMsg::new("msg_s", "ses_a")
    });

    let h = Harness::new("stream");
    h.scan(&db);
    assert_eq!(h.reader().count(), 1);

    // grow the same message row: more tokens, later time_updated.
    db.assistant(&AsstMsg {
        output: 90,
        cache_read: 400,
        time_updated: 1_771_000_000_200,
        completed: Some(1_771_000_000_200),
        ..AsstMsg::new("msg_s", "ses_a")
    });
    h.scan(&db);

    let r = h.reader().row("msg_s");
    assert_eq!(h.reader().count(), 1);
    assert_eq!((r.output, r.cache_read), (90, 400));
    assert_eq!(r.dirty, 1);

    // rescan with no upstream change -> zero new inserts, still one row.
    let s = h.scan(&db);
    assert_eq!(s.events_inserted, 0);
    assert_eq!(h.reader().count(), 1);
}

// --- cursor: >= boundary dedup -------------------------------------------

#[test]
fn boundary_millisecond_reread_dedups() {
    let db = OcDb::new("boundary");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    // two rows sharing the exact same time_updated.
    db.assistant(&AsstMsg {
        time_updated: 1_771_000_000_500,
        completed: Some(1_771_000_000_500),
        ..AsstMsg::new("msg_a", "ses_a")
    });
    db.assistant(&AsstMsg {
        time_updated: 1_771_000_000_500,
        completed: Some(1_771_000_000_500),
        ..AsstMsg::new("msg_b", "ses_a")
    });

    let h = Harness::new("boundary");
    let s1 = h.scan(&db);
    assert_eq!(s1.events_inserted, 2);

    // the cursor tu == 500; the next scan uses >= so it re-reads both rows,
    // but dedup absorbs them: zero new inserts.
    let s2 = h.scan(&db);
    assert_eq!(s2.events_inserted, 0);
    assert_eq!(h.reader().count(), 2);
}

// --- reset tripwires -----------------------------------------------------

#[test]
fn reset_on_db_path_change_rescans_without_duplicates() {
    let db1 = OcDb::new("path1");
    db1.project("prj_1", "/wt");
    db1.session("ses_a", "prj_1", "1.2.6");
    db1.assistant(&AsstMsg::new("msg_1", "ses_a"));

    let h = Harness::new("path");
    h.scan(&db1);
    assert_eq!(h.reader().count(), 1);

    // A different db path with the same message id: the path tripwire fires,
    // cursor restarts at 0, but the stable message.id key dedups to 0 inserts.
    let db2 = OcDb::new("path2");
    db2.project("prj_1", "/wt");
    db2.session("ses_a", "prj_1", "1.2.6");
    db2.assistant(&AsstMsg::new("msg_1", "ses_a"));
    let s = h.scan(&db2);
    assert_eq!(s.events_inserted, 0);
    assert_eq!(h.reader().count(), 1);
}

#[test]
fn reset_on_count_shrink_rescans_and_keeps_deleted_row() {
    let db = OcDb::new("shrink");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg::new("msg_1", "ses_a"));
    db.assistant(&AsstMsg { time_updated: 1_771_000_000_010, ..AsstMsg::new("msg_2", "ses_a") });

    let h = Harness::new("shrink");
    h.scan(&db);
    assert_eq!(h.reader().count(), 2);

    // upstream hard-delete shrinks COUNT -> tripwire fires -> full rescan. The
    // deleted row persists in the ledger (spend happened).
    db.delete_message("msg_2");
    let s = h.scan(&db);
    assert_eq!(s.events_inserted, 0);
    assert_eq!(h.reader().count(), 2);
}

#[test]
fn reset_on_min_id_change_rescans() {
    let db = OcDb::new("minid");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg::new("msg_5", "ses_a"));

    let h = Harness::new("minid");
    h.scan(&db);
    assert!(h.cursor().unwrap().contains("msg_5"));

    // a re-migrated history introduces an older min id with an OLD time_updated
    // (below the stored tu): the max-regression check alone would miss it, the
    // min-id anchor catches it.
    db.assistant(&AsstMsg {
        id: "msg_1",
        time_updated: 1_770_000_000_000,
        completed: Some(1_770_000_000_000),
        time_created: 1_770_000_000_000,
        ..AsstMsg::new("msg_1", "ses_a")
    });
    let s = h.scan(&db);
    // msg_1 is below the stored tu; without the reset it would never be seen.
    assert_eq!(s.events_inserted, 1);
    assert_eq!(h.reader().count(), 2);
}

#[test]
fn reset_on_max_tu_regression_rescans() {
    let db = OcDb::new("regress");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg {
        time_updated: 1_771_000_009_000,
        completed: Some(1_771_000_009_000),
        ..AsstMsg::new("msg_hi", "ses_a")
    });

    let h = Harness::new("regress");
    h.scan(&db);
    assert_eq!(h.reader().count(), 1);

    // DB recreated: same session, a much lower time_updated. MAX(tu) now
    // regresses below the stored tu -> reset.
    db.delete_message("msg_hi");
    db.assistant(&AsstMsg {
        id: "msg_lo",
        time_updated: 1_770_000_000_000,
        completed: Some(1_770_000_000_000),
        ..AsstMsg::new("msg_lo", "ses_a")
    });
    let s = h.scan(&db);
    assert_eq!(s.events_inserted, 1);
    // the deleted high row persists; the low row is added.
    assert_eq!(h.reader().count(), 2);
}

// --- schema drift --------------------------------------------------------

#[test]
fn schema_drift_missing_project_table_falls_back() {
    let db = OcDb::with_ddl("drift", OPENCODE_NO_PROJECT_DDL);
    db.session_bare("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg { cwd: "/from/cwd", ..AsstMsg::new("msg_1", "ses_a") });

    let h = Harness::new("drift");
    let s = h.scan(&db);
    assert_eq!(s.events_inserted, 1);

    // the fallback query has no join, so version is empty but tokens/project
    // (from data JSON) survive.
    let r = h.reader().row("msg_1");
    assert_eq!(r.project, "/from/cwd");
    assert_eq!(r.agent_version, "");
}

#[test]
fn malformed_data_json_is_counted_skipped() {
    let db = OcDb::new("malformed");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    // a good row plus a garbage-JSON row at a later time_updated.
    db.assistant(&AsstMsg::new("msg_ok", "ses_a"));
    db.upsert_raw("msg_bad", "ses_a", 1_771_000_000_010, 1_771_000_000_010, "{not valid json");

    let h = Harness::new("malformed");
    let s = h.scan(&db);
    assert_eq!(s.events_inserted, 1);
    assert_eq!(s.lines_skipped, 1);
}

// --- step-semantics: Branch 2 (per-step replacement) ---------------------

#[test]
fn branch2_in_flight_row_without_completed_is_gated() {
    let db = OcDb::new("inflight");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    // recent time_updated and NO time.completed -> in flight -> gated.
    let now = now_ms();
    db.assistant(&AsstMsg {
        time_created: now,
        time_updated: now,
        completed: None,
        ..AsstMsg::new("msg_live", "ses_a")
    });

    let h = Harness::new("inflight");
    h.scan(&db);
    // gated: not emitted while still in flight.
    assert_eq!(h.reader().count(), 0);

    // the cursor must not skip past the gated row: it stalls at (or below) the
    // gated row's time_updated so a later scan re-reads it.
    let tu = cursor_tu(&h.cursor().unwrap());
    assert!(tu <= now, "cursor must not advance past a gated row: {tu} > {now}");

    // the row completes -> it now passes the gate and is emitted.
    db.assistant(&AsstMsg {
        time_created: now,
        time_updated: now + 5,
        completed: Some(now + 5),
        ..AsstMsg::new("msg_live", "ses_a")
    });
    h.scan(&db);
    assert_eq!(h.reader().count(), 1);
}

#[test]
fn branch2_quiescent_in_flight_row_emits_after_grace() {
    let db = OcDb::new("quiescent");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    // old (well past the 10-minute grace) time_updated, still no completed (an
    // aborted row that never got `completed`): emitted anyway.
    let old = now_ms() - 3_600_000;
    db.assistant(&AsstMsg {
        time_created: old,
        time_updated: old,
        completed: None,
        ..AsstMsg::new("msg_stale", "ses_a")
    });

    let h = Harness::new("quiescent");
    h.scan(&db);
    assert_eq!(h.reader().count(), 1);
}

// --- billing -------------------------------------------------------------

#[test]
fn cost_rows_are_estimated_so_mode_stays_subscription() {
    let db = OcDb::new("billing");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg { cost: 0.42, ..AsstMsg::new("msg_1", "ses_a") });
    db.assistant(&AsstMsg {
        cost: 0.10,
        time_updated: 1_771_000_000_010,
        ..AsstMsg::new("msg_2", "ses_a")
    });

    let h = Harness::new("billing");
    h.scan(&db);
    // opencode's cost is a synthetic models.dev figure -> always Estimated ->
    // billing auto-detect stays subscription, never api.
    assert_eq!(h.billing_mode(), "subscription");
}

// --- idempotency ---------------------------------------------------------

#[test]
fn dogfood_sum_shape_holds_and_rescan_inserts_zero() {
    let db = OcDb::new("idemp");
    db.project("prj_1", "/wt");
    db.session("ses_a", "prj_1", "1.2.6");
    db.assistant(&AsstMsg {
        input: 46944,
        output: 20944,
        cache_read: 5020617,
        cache_write: 96263,
        reasoning: 0,
        cost: 0.4158,
        ..AsstMsg::new("msg_1", "ses_a")
    });

    let h = Harness::new("idemp");
    h.scan(&db);
    assert_eq!(h.reader().sums(), (46944, 20944, 5020617, 96263, 0));

    let s = h.scan(&db);
    assert_eq!(s.events_inserted, 0);
    assert_eq!(h.reader().count(), 1);
}
