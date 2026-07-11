use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;

use super::{DbAdapter, DbFetch, SourceCursorState};
use crate::model::{CostBasis, UsageEvent};

/// opencode SQLite adapter. Reads the `message` table of
/// `~/.local/share/opencode/opencode.db` (WAL since ~v1.2), incrementally
/// cursored on `time_updated`. Opened strictly read-only; the app updates a
/// message row in place on every stream step, so an `id`/rowid watermark would
/// miss those updates — `time_updated` is the only correct cursor.
pub struct OpencodeAdapter;

impl OpencodeAdapter {
    pub const SOURCE: &'static str = "opencode";
}

/// opencode assigns each message row a final `data.time.completed` after its
/// stream ends. Per-step usage is a REPLACEMENT (AI SDK v5 `usage` is the last
/// step only, not a cumulative total), so a mid-stream snapshot can exceed the
/// final row and strand keep-higher forever. An in-flight row is emitted only
/// once it has `time.completed`, or once it has been quiescent past this grace
/// window (covers aborted rows that never get `completed`).
const INFLIGHT_GRACE_MS: i64 = 600_000;

impl DbAdapter for OpencodeAdapter {
    fn source(&self) -> &'static str {
        Self::SOURCE
    }

    fn discover(&self) -> Option<PathBuf> {
        discover_from(
            std::env::var_os("RUNTAB_OPENCODE_DB").map(PathBuf::from),
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
            crate::home_dir(),
        )
    }

    fn fetch(&self, db_path: &Path, stored: Option<&SourceCursorState>) -> anyhow::Result<DbFetch> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening opencode db at {}", db_path.display()))?;
        // opencode writes to this db constantly while a session runs; a scan
        // racing a write would otherwise fail immediately on SQLITE_BUSY.
        conn.busy_timeout(Duration::from_millis(2000))
            .with_context(|| format!("setting busy_timeout on opencode db at {}", db_path.display()))?;

        let probe = probe_message(&conn, db_path)?;
        let stored_cursor = stored.map(|s| Cursor::parse(&s.cursor));
        let db_path_str = db_path.to_string_lossy();
        let tu_floor = cursor_floor(stored, stored_cursor.as_ref(), &probe, &db_path_str);

        let rows = read_rows(&conn, tu_floor, db_path)?;

        let now = crate::timeutil::now_epoch() * 1000;
        let mut events = Vec::new();
        let mut rows_skipped = 0u64;
        let mut max_tu_seen = tu_floor;
        let mut min_gated_tu: Option<i64> = None;

        for row in rows {
            if row.time_updated > max_tu_seen {
                max_tu_seen = row.time_updated;
            }
            let data: Value = match serde_json::from_str(&row.data) {
                Ok(v) => v,
                Err(_) => {
                    rows_skipped += 1;
                    continue;
                }
            };
            if data.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(tokens) = data.get("tokens").filter(|t| t.is_object()) else {
                rows_skipped += 1;
                continue;
            };
            let input = jint(tokens, "input");
            let output = jint(tokens, "output");
            let reasoning = jint(tokens, "reasoning");
            let cache = tokens.get("cache");
            let cache_read = cache.map(|c| jint(c, "read")).unwrap_or(0);
            let cache_write = cache.map(|c| jint(c, "write")).unwrap_or(0);
            if input + output + cache_read + cache_write + reasoning <= 0 {
                continue;
            }

            // Branch-2 gate: an in-flight row (no `time.completed`) that is still
            // recent may carry a mid-stream snapshot larger than its final total.
            let completed = data.pointer("/time/completed").and_then(Value::as_i64);
            if completed.is_none() && row.time_updated >= now - INFLIGHT_GRACE_MS {
                min_gated_tu =
                    Some(min_gated_tu.map_or(row.time_updated, |m| m.min(row.time_updated)));
                continue;
            }

            let cost = data.get("cost").and_then(Value::as_f64);
            let cost_usd = match cost {
                Some(c) if c > 0.0 => Some(c),
                _ => None,
            };

            events.push(UsageEvent {
                source: Self::SOURCE.to_string(),
                message_id: row.id.clone(),
                request_id: String::new(),
                session_id: row.session_id.clone(),
                ts: crate::timeutil::epoch_ms_to_rfc3339(row.time_created),
                model: data
                    .get("modelID")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .unwrap_or("unknown")
                    .to_string(),
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_creation_tokens: cache_write,
                cache_1h_tokens: 0,
                cache_5m_tokens: 0,
                reasoning_tokens: reasoning,
                project: project_of(&data, row.worktree.as_deref()),
                agent_version: row.version.clone().unwrap_or_default(),
                cost_usd,
                cost_basis: CostBasis::Estimated,
            });
        }

        // The cursor must never advance past a gated row, so it re-enters the
        // window next tick; `>=` boundary re-reads are absorbed by dedup.
        let new_tu = match min_gated_tu {
            Some(g) => g.min(max_tu_seen),
            None => max_tu_seen,
        };
        let new_cursor = Cursor {
            tu: new_tu,
            min_id: probe.min_id.clone(),
        };

        Ok(DbFetch {
            events,
            rows_skipped,
            new_cursor: new_cursor.encode(),
            row_count: probe.count,
        })
    }
}

/// `RUNTAB_OPENCODE_DB` override (authoritative — set-but-missing is `None`,
/// never a fall-through to a default, so a misconfigured override never reads a
/// developer's real db), else `$XDG_DATA_HOME/opencode/opencode.db`, else
/// `~/.local/share/opencode/opencode.db` (correct on macOS too — opencode uses
/// xdg-basedir with no macOS special case).
fn discover_from(
    override_path: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(path) = override_path {
        return path.exists().then_some(path);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(xdg) = xdg_data_home.filter(|p| !p.as_os_str().is_empty()) {
        candidates.push(xdg.join("opencode").join("opencode.db"));
    }
    if let Some(home) = &home {
        candidates.push(home.join(".local/share/opencode/opencode.db"));
    }
    candidates.into_iter().find(|p| p.exists())
}

struct Probe {
    count: i64,
    max_tu: i64,
    min_id: String,
}

fn probe_message(conn: &Connection, db_path: &Path) -> anyhow::Result<Probe> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(time_updated),0), COALESCE(MIN(id),'') FROM message",
        [],
        |r| {
            Ok(Probe {
                count: r.get(0)?,
                max_tu: r.get(1)?,
                min_id: r.get(2)?,
            })
        },
    )
    .with_context(|| format!("probing opencode message table at {}", db_path.display()))
}

/// Opaque cursor persisted in `source_cursors.cursor`: the max `time_updated`
/// consumed and the oldest `message.id` seen (a re-migration anchor).
struct Cursor {
    tu: i64,
    min_id: String,
}

impl Cursor {
    fn parse(s: &str) -> Cursor {
        let v: Value = serde_json::from_str(s).unwrap_or(Value::Null);
        Cursor {
            tu: v.get("tu").and_then(Value::as_i64).unwrap_or(0),
            min_id: v.get("min_id").and_then(Value::as_str).unwrap_or("").to_string(),
        }
    }

    fn encode(&self) -> String {
        serde_json::json!({ "tu": self.tu, "min_id": self.min_id }).to_string()
    }
}

/// Reset tripwires (§5.3): any restarts the cursor at 0 (a full rescan is
/// idempotent — the stable `message.id` key dedups to zero inserts). Catches a
/// moved db, a shrunk table (hard-deletes/recreation), a regressed max
/// `time_updated`, and a re-migrated history whose oldest id changed while its
/// `time_updated` values stayed below the stored watermark.
fn cursor_floor(
    stored: Option<&SourceCursorState>,
    cursor: Option<&Cursor>,
    probe: &Probe,
    db_path: &str,
) -> i64 {
    let Some(stored) = stored else { return 0 };
    let Some(cursor) = cursor else { return 0 };
    let reset = stored.db_path != db_path
        || probe.max_tu < cursor.tu
        || probe.count < stored.row_count
        || (!cursor.min_id.is_empty() && cursor.min_id != probe.min_id);
    if reset {
        0
    } else {
        cursor.tu
    }
}

struct Row {
    id: String,
    session_id: String,
    time_created: i64,
    time_updated: i64,
    data: String,
    version: Option<String>,
    worktree: Option<String>,
}

/// Rich query first (joins `session.version` and `project.worktree`); on a
/// prepare error (schema drift on a newer opencode — real column churn exists
/// upstream) fall back to the core columns only.
fn read_rows(conn: &Connection, tu_floor: i64, db_path: &Path) -> anyhow::Result<Vec<Row>> {
    const RICH: &str = "SELECT m.id, m.session_id, m.time_created, m.time_updated, m.data,
                               s.version, p.worktree
                        FROM message m
                        LEFT JOIN session s ON s.id = m.session_id
                        LEFT JOIN project p ON p.id = s.project_id
                        WHERE m.time_updated >= ?1
                        ORDER BY m.time_updated ASC, m.id ASC";
    const BARE: &str = "SELECT id, session_id, time_created, time_updated, data
                        FROM message WHERE time_updated >= ?1
                        ORDER BY time_updated ASC, id ASC";

    if let Ok(mut stmt) = conn.prepare(RICH) {
        let rows = stmt
            .query_map(params![tu_floor], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    time_created: r.get(2)?,
                    time_updated: r.get(3)?,
                    data: r.get(4)?,
                    version: r.get(5)?,
                    worktree: r.get(6)?,
                })
            })
            .with_context(|| format!("querying opencode messages at {}", db_path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| format!("reading opencode messages at {}", db_path.display()))?;
        return Ok(rows);
    }

    let mut stmt = conn
        .prepare(BARE)
        .with_context(|| format!("preparing opencode fallback query at {}", db_path.display()))?;
    let rows = stmt
        .query_map(params![tu_floor], |r| {
            Ok(Row {
                id: r.get(0)?,
                session_id: r.get(1)?,
                time_created: r.get(2)?,
                time_updated: r.get(3)?,
                data: r.get(4)?,
                version: None,
                worktree: None,
            })
        })
        .with_context(|| format!("querying opencode fallback messages at {}", db_path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("reading opencode fallback messages at {}", db_path.display()))?;
    Ok(rows)
}

fn jint(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// `data.path.cwd` → `data.path.root` → the joined `project.worktree` → `""`.
fn project_of(data: &Value, worktree: Option<&str>) -> String {
    let cwd = data.pointer("/path/cwd").and_then(Value::as_str).unwrap_or("");
    if !cwd.is_empty() {
        return cwd.to_string();
    }
    let root = data.pointer("/path/root").and_then(Value::as_str).unwrap_or("");
    if !root.is_empty() {
        return root.to_string();
    }
    worktree.unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_prefers_override_then_xdg_then_home() {
        let dir = std::env::temp_dir().join(format!("rt_oc_disc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let xdg = dir.join("xdg");
        let home = dir.join("home");
        std::fs::create_dir_all(xdg.join("opencode")).unwrap();
        std::fs::create_dir_all(home.join(".local/share/opencode")).unwrap();
        let xdg_db = xdg.join("opencode/opencode.db");
        let home_db = home.join(".local/share/opencode/opencode.db");
        std::fs::write(&xdg_db, b"").unwrap();
        std::fs::write(&home_db, b"").unwrap();

        assert_eq!(
            discover_from(None, Some(xdg.clone()), Some(home.clone())),
            Some(xdg_db.clone())
        );
        assert_eq!(discover_from(None, None, Some(home.clone())), Some(home_db));

        let over = dir.join("explicit.db");
        std::fs::write(&over, b"").unwrap();
        assert_eq!(
            discover_from(Some(over.clone()), Some(xdg), Some(home)),
            Some(over)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_set_but_missing_never_falls_through() {
        let dir = std::env::temp_dir().join(format!("rt_oc_disc2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        std::fs::create_dir_all(home.join(".local/share/opencode")).unwrap();
        std::fs::write(home.join(".local/share/opencode/opencode.db"), b"").unwrap();
        let missing = dir.join("nope.db");
        assert_eq!(discover_from(Some(missing), None, Some(home)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
