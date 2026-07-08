//! Imports rows from rtk's own SQLite ledger (`~/.local/share/rtk/history.db`)
//! into our `rtk_events` table, behind a monotonic row-id cursor persisted in
//! `rtk_scan_state`. rtk is a separate, unrelated tool (a token-saving CLI
//! proxy) that keeps its own append-only ledger of shell commands it ran on
//! our behalf; this module lets us report the savings without ever storing
//! the commands themselves — only sha256 hashes via `crate::cmdnorm`.
//!
//! Read-only on rtk's db: we never know when rtk itself is about to write to
//! it, so the connection opens `SQLITE_OPEN_READ_ONLY` and never issues a
//! write statement against it.

mod attribute;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;

use crate::cmdnorm;
use crate::ledger::Ledger;
use crate::model::RtkCommandRow;

pub use attribute::{attribute, AttributionSummary};

const BATCH_SIZE: i64 = 5000;

/// Summary of one `import` run.
#[derive(Debug, Default, Serialize)]
pub struct RtkImportSummary {
    pub rows_imported: u64,
    pub cursor: i64,
}

/// Locates rtk's history db: `$RUNTAB_RTK_DB` override, then
/// `$XDG_DATA_HOME/rtk/history.db`, then `~/.local/share/rtk/history.db`,
/// then the macOS `~/Library/Application Support/rtk/history.db` path.
/// An override is authoritative — if it's set but the path doesn't exist,
/// that's `None`, never a silent fall-through to the default candidates (a
/// misconfigured `RUNTAB_RTK_DB` must not import from the user's real rtk
/// db). Defaults are only consulted when no override is set at all; among
/// those, the first candidate that exists wins. `None` otherwise means rtk
/// isn't installed (or hasn't run yet) on this machine.
pub fn discover_db() -> Option<PathBuf> {
    discover_db_from(
        std::env::var_os("RUNTAB_RTK_DB").map(PathBuf::from),
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        crate::home_dir(),
    )
}

fn discover_db_from(
    override_path: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(path) = override_path {
        return path.exists().then_some(path);
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(xdg) = xdg_data_home.filter(|p| !p.as_os_str().is_empty()) {
        candidates.push(xdg.join("rtk").join("history.db"));
    }
    if let Some(home) = &home {
        candidates.push(home.join(".local/share/rtk/history.db"));
        candidates.push(home.join("Library/Application Support/rtk/history.db"));
    }
    candidates.into_iter().find(|p| p.exists())
}

/// Discover-and-import in one call. `Ok(None)` means rtk isn't present on
/// this machine, which is normal, not an error — the feature is silently off.
pub fn run(ledger: &Ledger) -> anyhow::Result<Option<RtkImportSummary>> {
    run_with(ledger, discover_db())
}

/// Split out from `run` so the `db_path` it acts on can be handed in directly
/// in tests, instead of reaching for process-global env vars.
fn run_with(ledger: &Ledger, db_path: Option<PathBuf>) -> anyhow::Result<Option<RtkImportSummary>> {
    match db_path {
        None => Ok(None),
        Some(path) => import(ledger, &path).map(Some),
    }
}

/// Import every rtk row with `id` past our stored cursor, in batches, each
/// batch committed atomically against our ledger (see `apply_batch`).
pub fn import(ledger: &Ledger, db_path: &Path) -> anyhow::Result<RtkImportSummary> {
    import_batched(ledger, db_path, BATCH_SIZE)
}

fn import_batched(ledger: &Ledger, db_path: &Path, batch_size: i64) -> anyhow::Result<RtkImportSummary> {
    let rtk_conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening rtk db at {}", db_path.display()))?;
    // rtk writes to this db constantly (every filtered command); without this
    // a scan racing an rtk invocation would fail immediately on SQLITE_BUSY.
    rtk_conn
        .busy_timeout(Duration::from_millis(2000))
        .with_context(|| format!("setting busy_timeout on rtk db at {}", db_path.display()))?;

    let db_path_str = db_path.to_string_lossy().into_owned();
    let max_id: Option<i64> = rtk_conn
        .query_row("SELECT MAX(id) FROM commands", [], |r| r.get(0))
        .with_context(|| format!("reading MAX(id) from rtk db at {}", db_path.display()))?;

    let mut cursor = match ledger.rtk_scan_cursor()? {
        Some((stored_path, last_row_id)) if stored_path == db_path_str && max_id.unwrap_or(0) >= last_row_id => {
            last_row_id
        }
        Some(_) => {
            // Either a different rtk db than last time, or `rtk --reset` wiped
            // the table and ids restarted lower than our cursor. Old imported
            // rows stay (see `insert_rtk_event`); we just stop assuming
            // monotonic ids from here. The attribution watermark is *not*
            // reset alongside this cursor: it tracks our own `rtk_events.id`
            // (an internal, ever-growing primary key), which this reset
            // doesn't touch — only rtk's own row ids restart.
            eprintln!("runtab: rtk history looks new/reset; restarting import cursor");
            0
        }
        None => 0,
    };

    let mut summary = RtkImportSummary { rows_imported: 0, cursor };
    loop {
        let rows = fetch_batch(&rtk_conn, cursor, batch_size, db_path)?;
        let Some(&last_id) = rows.last().map(|r| &r.rtk_row_id) else {
            break;
        };
        let batch_len = rows.len() as i64;

        summary.rows_imported += apply_batch(ledger, &db_path_str, &rows, last_id)?;
        cursor = last_id;
        summary.cursor = cursor;

        if batch_len < batch_size {
            break;
        }
    }

    Ok(summary)
}

fn fetch_batch(
    rtk_conn: &Connection,
    cursor: i64,
    batch_size: i64,
    db_path: &Path,
) -> anyhow::Result<Vec<RtkCommandRow>> {
    let mut stmt = rtk_conn
        .prepare(
            "SELECT id, timestamp, original_cmd, input_tokens, output_tokens, saved_tokens,
                    exec_time_ms, project_path
             FROM commands WHERE id > ?1 ORDER BY id LIMIT ?2",
        )
        .with_context(|| format!("preparing batch query on rtk db at {}", db_path.display()))?;
    let rows = stmt
        .query_map(params![cursor, batch_size], |row| {
            let original_cmd: String = row.get(2)?;
            Ok(RtkCommandRow {
                rtk_row_id: row.get(0)?,
                ts: row.get(1)?,
                head_hash: cmdnorm::hash(&cmdnorm::head(&original_cmd)),
                cmd_hash: cmdnorm::hash(&original_cmd),
                raw_tokens: row.get(3)?,
                filtered_tokens: row.get(4)?,
                saved_tokens: row.get(5)?,
                exec_time_ms: row.get(6)?,
                project_path: row.get(7)?,
            })
        })
        .with_context(|| format!("querying batch rows from rtk db at {}", db_path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("reading batch rows from rtk db at {}", db_path.display()))?;
    Ok(rows)
}

/// Insert `rows` and advance the cursor in one transaction on our ledger: a
/// crash between batches must not lose or duplicate rows on rerun, and
/// `INSERT OR IGNORE` + cursor-in-tx gives exactly that.
fn apply_batch(
    ledger: &Ledger,
    db_path_str: &str,
    rows: &[RtkCommandRow],
    last_id: i64,
) -> anyhow::Result<u64> {
    ledger.tx_begin()?;
    let mut imported = 0u64;
    for row in rows {
        match ledger.insert_rtk_event(row) {
            Ok(true) => imported += 1,
            Ok(false) => {}
            Err(e) => {
                let _ = ledger.tx_rollback();
                return Err(e.into());
            }
        }
    }
    if let Err(e) = ledger.set_rtk_scan_cursor(db_path_str, last_id) {
        let _ = ledger.tx_rollback();
        return Err(e.into());
    }
    if let Err(e) = ledger.tx_commit() {
        let _ = ledger.tx_rollback();
        return Err(e.into());
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    const RTK_DDL: &str = "
    CREATE TABLE commands (
        id INTEGER PRIMARY KEY,
        timestamp TEXT NOT NULL,
        original_cmd TEXT NOT NULL,
        rtk_cmd TEXT NOT NULL,
        input_tokens INTEGER NOT NULL,
        output_tokens INTEGER NOT NULL,
        saved_tokens INTEGER NOT NULL,
        savings_pct REAL NOT NULL,
        exec_time_ms INTEGER DEFAULT 0,
        project_path TEXT DEFAULT ''
    );";

    fn temp_path(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("runtab_rtkimport_{prefix}_{}_{nanos}_{unique}.db", std::process::id()))
    }

    fn fixture_conn(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(RTK_DDL).unwrap();
        conn
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_row(
        conn: &Connection,
        id: i64,
        ts: &str,
        original_cmd: &str,
        input_tokens: i64,
        output_tokens: i64,
        saved_tokens: i64,
        exec_time_ms: i64,
        project_path: &str,
    ) {
        conn.execute(
            "INSERT INTO commands
                (id, timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                 saved_tokens, savings_pct, exec_time_ms, project_path)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, 0.0, ?7, ?8)",
            params![id, ts, original_cmd, input_tokens, output_tokens, saved_tokens, exec_time_ms, project_path],
        )
        .unwrap();
    }

    struct StoredEvent {
        rtk_row_id: i64,
        ts: String,
        project_path: String,
        head_hash: String,
        cmd_hash: String,
        raw_tokens: i64,
        filtered_tokens: i64,
        saved_tokens: i64,
        exec_time_ms: i64,
    }

    fn rtk_events(ledger_path: &Path) -> Vec<StoredEvent> {
        let conn = Connection::open(ledger_path).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT rtk_row_id, ts, project_path, head_hash, cmd_hash,
                        raw_tokens, filtered_tokens, saved_tokens, exec_time_ms
                 FROM rtk_events ORDER BY rtk_row_id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok(StoredEvent {
                rtk_row_id: r.get(0)?,
                ts: r.get(1)?,
                project_path: r.get(2)?,
                head_hash: r.get(3)?,
                cmd_hash: r.get(4)?,
                raw_tokens: r.get(5)?,
                filtered_tokens: r.get(6)?,
                saved_tokens: r.get(7)?,
                exec_time_ms: r.get(8)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    fn cleanup(paths: &[&Path]) {
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn imports_from_cursor_zero_and_never_stores_raw_command_text() {
        let rtk_path = temp_path("rtkdb");
        let ledger_path = temp_path("ledger");
        {
            let conn = fixture_conn(&rtk_path);
            insert_row(&conn, 1, "2026-07-01T00:00:00.123+00:00", "git status", 500, 100, 400, 12, "/home/u/proj");
        }

        let ledger = Ledger::open(&ledger_path).unwrap();
        let summary = import(&ledger, &rtk_path).unwrap();
        drop(ledger);

        assert_eq!(summary.rows_imported, 1);
        assert_eq!(summary.cursor, 1);

        let rows = rtk_events(&ledger_path);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cmd_hash, cmdnorm::hash("git status"));
        assert_eq!(rows[0].head_hash, cmdnorm::hash(&cmdnorm::head("git status")));

        assert!(!rows[0].ts.contains("git status"));
        assert!(!rows[0].project_path.contains("git status"));
        assert!(!rows[0].head_hash.contains("git status"));
        assert!(!rows[0].cmd_hash.contains("git status"));

        cleanup(&[&rtk_path, &ledger_path]);
    }

    #[test]
    fn second_import_with_no_new_rows_imports_nothing_and_keeps_cursor() {
        let rtk_path = temp_path("rtkdb");
        let ledger_path = temp_path("ledger");
        {
            let conn = fixture_conn(&rtk_path);
            insert_row(&conn, 1, "2026-07-01T00:00:00+00:00", "git status", 500, 100, 400, 12, "/home/u/proj");
        }

        let ledger = Ledger::open(&ledger_path).unwrap();
        let first = import(&ledger, &rtk_path).unwrap();
        assert_eq!(first.cursor, 1);

        let second = import(&ledger, &rtk_path).unwrap();
        drop(ledger);

        assert_eq!(second.rows_imported, 0);
        assert_eq!(second.cursor, 1);
        assert_eq!(rtk_events(&ledger_path).len(), 1);

        cleanup(&[&rtk_path, &ledger_path]);
    }

    #[test]
    fn only_the_delta_imports_after_new_rtk_rows_land() {
        let rtk_path = temp_path("rtkdb");
        let ledger_path = temp_path("ledger");
        {
            let conn = fixture_conn(&rtk_path);
            insert_row(&conn, 1, "2026-07-01T00:00:00+00:00", "git status", 500, 100, 400, 12, "/home/u/proj");
        }

        {
            let ledger = Ledger::open(&ledger_path).unwrap();
            let first = import(&ledger, &rtk_path).unwrap();
            assert_eq!(first.rows_imported, 1);
        }

        {
            let conn = Connection::open(&rtk_path).unwrap();
            insert_row(&conn, 2, "2026-07-01T00:01:00+00:00", "git log", 300, 60, 240, 8, "/home/u/proj");
        }

        // Reopen the ledger to prove the cursor is durable, not carried in-process.
        let ledger = Ledger::open(&ledger_path).unwrap();
        let second = import(&ledger, &rtk_path).unwrap();
        drop(ledger);

        assert_eq!(second.rows_imported, 1);
        assert_eq!(second.cursor, 2);
        let rows = rtk_events(&ledger_path);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].cmd_hash, cmdnorm::hash("git log"));

        cleanup(&[&rtk_path, &ledger_path]);
    }

    #[test]
    fn maps_every_token_and_metadata_field() {
        let rtk_path = temp_path("rtkdb");
        let ledger_path = temp_path("ledger");
        {
            let conn = fixture_conn(&rtk_path);
            insert_row(
                &conn,
                7,
                "2026-07-01T12:34:56.789+00:00",
                "cargo test -p runtab",
                1234,
                88,
                1146,
                4321,
                "/home/u/proj/runtab",
            );
        }

        let ledger = Ledger::open(&ledger_path).unwrap();
        import(&ledger, &rtk_path).unwrap();
        drop(ledger);

        let rows = rtk_events(&ledger_path);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.rtk_row_id, 7);
        assert_eq!(row.ts, "2026-07-01T12:34:56.789+00:00");
        assert_eq!(row.project_path, "/home/u/proj/runtab");
        assert_eq!(row.raw_tokens, 1234);
        assert_eq!(row.filtered_tokens, 88);
        assert_eq!(row.saved_tokens, 1146);
        assert_eq!(row.exec_time_ms, 4321);
        assert_eq!(row.cmd_hash, cmdnorm::hash("cargo test -p runtab"));

        cleanup(&[&rtk_path, &ledger_path]);
    }

    #[test]
    fn run_with_no_db_path_is_a_silent_noop() {
        let ledger = Ledger::open_in_memory().unwrap();
        let result = run_with(&ledger, None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn run_with_a_db_path_imports() {
        let rtk_path = temp_path("rtkdb");
        let ledger_path = temp_path("ledger");
        {
            let conn = fixture_conn(&rtk_path);
            insert_row(&conn, 1, "2026-07-01T00:00:00+00:00", "git status", 500, 100, 400, 12, "/home/u/proj");
        }

        let ledger = Ledger::open(&ledger_path).unwrap();
        let result = run_with(&ledger, Some(rtk_path.clone())).unwrap();
        drop(ledger);

        assert_eq!(result.unwrap().rows_imported, 1);
        cleanup(&[&rtk_path, &ledger_path]);
    }

    #[test]
    fn discover_db_from_prefers_override_then_xdg_then_home() {
        let dir = std::env::temp_dir().join(format!(
            "runtab_rtkimport_discover_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let xdg_dir = dir.join("xdg");
        let home_dir = dir.join("home");
        std::fs::create_dir_all(xdg_dir.join("rtk")).unwrap();
        std::fs::create_dir_all(home_dir.join(".local/share/rtk")).unwrap();

        // Neither candidate exists yet.
        assert_eq!(discover_db_from(None, Some(xdg_dir.clone()), Some(home_dir.clone())), None);

        // XDG candidate appears: picked over the (still-missing) home candidate.
        let xdg_db = xdg_dir.join("rtk").join("history.db");
        std::fs::write(&xdg_db, b"").unwrap();
        assert_eq!(
            discover_db_from(None, Some(xdg_dir.clone()), Some(home_dir.clone())),
            Some(xdg_db.clone())
        );

        // An override that exists wins even though the XDG candidate also exists.
        let override_db = dir.join("override.db");
        std::fs::write(&override_db, b"").unwrap();
        assert_eq!(
            discover_db_from(Some(override_db.clone()), Some(xdg_dir), Some(home_dir)),
            Some(override_db)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_db_from_override_set_but_missing_never_falls_through_to_a_default() {
        let dir = std::env::temp_dir().join(format!(
            "runtab_rtkimport_discover_override_missing_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home_dir = dir.join("home");
        std::fs::create_dir_all(home_dir.join(".local/share/rtk")).unwrap();
        let default_db = home_dir.join(".local/share/rtk/history.db");
        std::fs::write(&default_db, b"").unwrap();

        let missing_override = dir.join("does_not_exist.db");
        assert_eq!(discover_db_from(Some(missing_override), None, Some(home_dir)), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_detection_restarts_cursor_without_duplicating_or_losing_rows() {
        let rtk_path = temp_path("rtkdb");
        let ledger_path = temp_path("ledger");
        {
            let conn = fixture_conn(&rtk_path);
            insert_row(&conn, 1, "2026-07-01T00:00:00+00:00", "git status", 500, 100, 400, 12, "/home/u/proj");
            insert_row(&conn, 2, "2026-07-01T00:01:00+00:00", "git log", 300, 60, 240, 8, "/home/u/proj");
        }

        {
            let ledger = Ledger::open(&ledger_path).unwrap();
            let first = import(&ledger, &rtk_path).unwrap();
            assert_eq!(first.rows_imported, 2);
        }

        // Simulate `rtk --reset`: table wiped, ids restart from 1.
        {
            let conn = Connection::open(&rtk_path).unwrap();
            conn.execute("DELETE FROM commands", []).unwrap();
            insert_row(&conn, 1, "2026-07-02T00:00:00+00:00", "git status", 10, 2, 8, 1, "/home/u/proj");
        }

        let ledger = Ledger::open(&ledger_path).unwrap();
        let second = import(&ledger, &rtk_path).unwrap();
        drop(ledger);

        // The colliding rtk_row_id=1 is absorbed by INSERT OR IGNORE: the
        // original two rows survive untouched, nothing new lands for it.
        assert_eq!(second.rows_imported, 0);
        let rows = rtk_events(&ledger_path);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts, "2026-07-01T00:00:00+00:00");

        cleanup(&[&rtk_path, &ledger_path]);
    }

    #[test]
    fn batch_loop_crosses_the_configured_boundary() {
        let rtk_path = temp_path("rtkdb");
        let ledger_path = temp_path("ledger");
        {
            let conn = fixture_conn(&rtk_path);
            for id in 1..=5 {
                insert_row(&conn, id, "2026-07-01T00:00:00+00:00", "git status", 10, 2, 8, 1, "/home/u/proj");
            }
        }

        let ledger = Ledger::open(&ledger_path).unwrap();
        let summary = import_batched(&ledger, &rtk_path, 2).unwrap();
        drop(ledger);

        assert_eq!(summary.rows_imported, 5);
        assert_eq!(summary.cursor, 5);
        assert_eq!(rtk_events(&ledger_path).len(), 5);

        cleanup(&[&rtk_path, &ledger_path]);
    }
}
