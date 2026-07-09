//! Cross-scan pairing of `tool_use`/`tool_result` blocks into `tool_events`.
//!
//! A `tool_use` and its `tool_result` can land in different scan batches (the
//! transcript was only partially flushed at scan time). `pending_tool_calls`
//! stages a `tool_use` until its result shows up — in the same batch or a
//! later one — at which point the pair becomes one `tool_events` row.

use rusqlite::{params, OptionalExtension};

use super::Ledger;
use crate::model::{BashCandidate, ToolResultSeen, ToolUseSeen};

/// A staged `tool_use` row, read back out of `pending_tool_calls` to build
/// the finished `tool_events` row once its result arrives.
struct PendingRow {
    project: String,
    tool_name: String,
    est_args_tokens: i64,
    bash_head_hashes: Option<String>,
    bash_chain_hashes: Option<String>,
}

impl Ledger {
    /// Stage a `tool_use` awaiting its result. `INSERT OR REPLACE` on the
    /// `(source, session_id, tool_use_id)` primary key: a replayed scan
    /// (offset reset) re-stages the same row instead of erroring.
    pub fn insert_pending_tool_use(&self, t: &ToolUseSeen) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO pending_tool_calls
                (source, session_id, tool_use_id, ts, project, tool_name,
                 est_args_tokens, bash_head_hashes, bash_chain_hashes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                t.source,
                t.session_id,
                t.tool_use_id,
                t.ts,
                t.project,
                t.tool_name,
                t.est_args_tokens,
                t.bash_head_hashes,
                t.bash_chain_hashes,
            ],
        )?;
        Ok(())
    }

    /// Pair a `tool_result` with its staged `tool_use`, if any. A result with
    /// no matching pending row (interrupted or rotated transcript) is
    /// dropped silently. Returns whether a new `tool_events` row was
    /// actually inserted — `false` on a duplicate replay caught by
    /// `INSERT OR IGNORE`, so callers can count real insertions only.
    pub fn resolve_tool_result(&self, r: &ToolResultSeen) -> rusqlite::Result<bool> {
        let pending: Option<PendingRow> = self
            .conn
            .query_row(
                "SELECT project, tool_name, est_args_tokens, bash_head_hashes, bash_chain_hashes
                 FROM pending_tool_calls
                 WHERE source = ?1 AND session_id = ?2 AND tool_use_id = ?3",
                params![r.source, r.session_id, r.tool_use_id],
                |row| {
                    Ok(PendingRow {
                        project: row.get(0)?,
                        tool_name: row.get(1)?,
                        est_args_tokens: row.get(2)?,
                        bash_head_hashes: row.get(3)?,
                        bash_chain_hashes: row.get(4)?,
                    })
                },
            )
            .optional()?;

        let Some(pending) = pending else {
            return Ok(false);
        };

        // ts = the result's ts (completion time): the later rtk attribution
        // matcher compares against rtk's completion timestamps.
        self.conn.execute(
            "INSERT OR IGNORE INTO tool_events
                (source, session_id, tool_use_id, ts, project, tool_name,
                 est_args_tokens, est_result_tokens, is_error,
                 bash_head_hashes, bash_chain_hashes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                r.source,
                r.session_id,
                r.tool_use_id,
                r.ts,
                pending.project,
                pending.tool_name,
                pending.est_args_tokens,
                r.est_result_tokens,
                r.is_error,
                pending.bash_head_hashes,
                pending.bash_chain_hashes,
            ],
        )?;
        let inserted = self.conn.changes() > 0;

        self.conn.execute(
            "DELETE FROM pending_tool_calls
             WHERE source = ?1 AND session_id = ?2 AND tool_use_id = ?3",
            params![r.source, r.session_id, r.tool_use_id],
        )?;

        Ok(inserted)
    }

    /// Drop pending `tool_use` rows older than `before_ts` (RFC 3339,
    /// string-comparable) — a call whose result will never arrive
    /// (interrupted session, rotated transcript). No index on
    /// `pending_tool_calls.ts`: the table stays small by design, since rows
    /// are normally consumed as soon as their result lands.
    pub fn prune_pending(&self, before_ts: &str) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM pending_tool_calls WHERE ts < ?1",
            params![before_ts],
        )
    }

    /// Cheap backlog signal surfaced in `render_scan_summary`: `tool_use`
    /// blocks still waiting on a result. A growing count across scans usually
    /// means interrupted sessions or a stuck adapter, since rows are normally
    /// consumed within the same or next scan.
    pub fn pending_tool_calls_count(&self) -> rusqlite::Result<u64> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM pending_tool_calls", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }

    /// `Bash` `tool_events` whose completion `ts` (transcript format) falls
    /// in `[lower, upper]` — the widened SQL prefilter for rtk attribution
    /// (see `crate::rtkimport::attribute`, which narrows to the exact
    /// window by parsed epoch afterward). Uses `idx_tool_events_bash_ts`.
    pub fn bash_candidates_in_range(&self, lower: &str, upper: &str) -> rusqlite::Result<Vec<BashCandidate>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, session_id, project, ts, bash_head_hashes, bash_chain_hashes
             FROM tool_events
             WHERE tool_name = 'Bash' AND ts >= ?1 AND ts <= ?2",
        )?;
        let rows = stmt
            .query_map(params![lower, upper], |r| {
                Ok(BashCandidate {
                    id: r.get(0)?,
                    source: r.get(1)?,
                    session_id: r.get(2)?,
                    project: r.get(3)?,
                    ts: r.get(4)?,
                    bash_head_hashes: r.get(5)?,
                    bash_chain_hashes: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
impl Ledger {
    /// Test helper: internal `tool_events.id` for a known
    /// `(source, session_id, tool_use_id)`, used by attribution tests to
    /// pin an exact match rather than just a session.
    pub fn tool_event_id(&self, source: &str, session_id: &str, tool_use_id: &str) -> rusqlite::Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM tool_events WHERE source = ?1 AND session_id = ?2 AND tool_use_id = ?3",
                params![source, session_id, tool_use_id],
                |r| r.get(0),
            )
            .optional()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn use_seen(id: &str, session: &str) -> ToolUseSeen {
        ToolUseSeen {
            source: "claude_code".to_string(),
            session_id: session.to_string(),
            tool_use_id: id.to_string(),
            ts: "2026-07-01T00:00:00Z".to_string(),
            project: "/home/u/p".to_string(),
            tool_name: "Bash".to_string(),
            est_args_tokens: 3,
            bash_head_hashes: Some(r#"["h1"]"#.to_string()),
            bash_chain_hashes: Some(r#"["c1"]"#.to_string()),
        }
    }

    fn result_seen(id: &str, session: &str, ts: &str) -> ToolResultSeen {
        ToolResultSeen {
            source: "claude_code".to_string(),
            session_id: session.to_string(),
            tool_use_id: id.to_string(),
            ts: ts.to_string(),
            est_result_tokens: 7,
            is_error: false,
        }
    }

    fn tool_events_count(l: &Ledger) -> i64 {
        l.conn
            .query_row("SELECT COUNT(*) FROM tool_events", [], |r| r.get(0))
            .unwrap()
    }

    fn pending_count(l: &Ledger) -> i64 {
        l.conn
            .query_row("SELECT COUNT(*) FROM pending_tool_calls", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn same_batch_pairing_yields_one_row_and_empty_pending() {
        let l = Ledger::open_in_memory().unwrap();
        l.insert_pending_tool_use(&use_seen("toolu_1", "s1"))
            .unwrap();
        let inserted = l
            .resolve_tool_result(&result_seen("toolu_1", "s1", "2026-07-01T00:00:05Z"))
            .unwrap();

        assert!(inserted);
        assert_eq!(tool_events_count(&l), 1);
        assert_eq!(pending_count(&l), 0);
    }

    #[test]
    fn use_only_leaves_a_pending_row() {
        let l = Ledger::open_in_memory().unwrap();
        l.insert_pending_tool_use(&use_seen("toolu_2", "s1"))
            .unwrap();

        assert_eq!(tool_events_count(&l), 0);
        assert_eq!(pending_count(&l), 1);
    }

    #[test]
    fn result_in_a_later_batch_consumes_the_pending_row() {
        let l = Ledger::open_in_memory().unwrap();
        l.insert_pending_tool_use(&use_seen("toolu_3", "s1"))
            .unwrap(); // batch 1

        let inserted = l
            .resolve_tool_result(&result_seen("toolu_3", "s1", "2026-07-01T00:00:09Z"))
            .unwrap(); // batch 2

        assert!(inserted);
        assert_eq!(tool_events_count(&l), 1);
        assert_eq!(pending_count(&l), 0);
    }

    #[test]
    fn duplicate_replay_after_offset_reset_keeps_one_row() {
        let l = Ledger::open_in_memory().unwrap();
        l.insert_pending_tool_use(&use_seen("toolu_4", "s1"))
            .unwrap();
        assert!(l
            .resolve_tool_result(&result_seen("toolu_4", "s1", "2026-07-01T00:00:05Z"))
            .unwrap());

        // Offset reset: the same use+result pair is parsed and replayed.
        l.insert_pending_tool_use(&use_seen("toolu_4", "s1"))
            .unwrap();
        let inserted_again = l
            .resolve_tool_result(&result_seen("toolu_4", "s1", "2026-07-01T00:00:05Z"))
            .unwrap();

        assert!(!inserted_again);
        assert_eq!(tool_events_count(&l), 1);
        assert_eq!(pending_count(&l), 0);
    }

    #[test]
    fn result_with_no_pending_use_is_dropped_silently() {
        let l = Ledger::open_in_memory().unwrap();
        let inserted = l
            .resolve_tool_result(&result_seen("toolu_ghost", "s1", "2026-07-01T00:00:05Z"))
            .unwrap();

        assert!(!inserted);
        assert_eq!(tool_events_count(&l), 0);
        assert_eq!(pending_count(&l), 0);
    }

    #[test]
    fn resolve_stores_the_results_ts_not_the_uses_ts() {
        let l = Ledger::open_in_memory().unwrap();
        l.insert_pending_tool_use(&use_seen("toolu_5", "s1"))
            .unwrap(); // ts = ...T00:00:00Z
        l.resolve_tool_result(&result_seen("toolu_5", "s1", "2026-07-01T00:05:00Z"))
            .unwrap();

        let ts: String = l
            .conn
            .query_row(
                "SELECT ts FROM tool_events WHERE tool_use_id = 'toolu_5'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ts, "2026-07-01T00:05:00Z");
    }

    #[test]
    fn prune_removes_only_pending_rows_older_than_cutoff() {
        let l = Ledger::open_in_memory().unwrap();
        let mut old = use_seen("toolu_old", "s1");
        old.ts = "2026-06-01T00:00:00Z".to_string();
        let mut recent = use_seen("toolu_recent", "s1");
        recent.ts = "2026-07-01T00:00:00Z".to_string();
        l.insert_pending_tool_use(&old).unwrap();
        l.insert_pending_tool_use(&recent).unwrap();

        let deleted = l.prune_pending("2026-06-15T00:00:00Z").unwrap();

        assert_eq!(deleted, 1);
        assert_eq!(pending_count(&l), 1);
        let remaining: String = l
            .conn
            .query_row("SELECT tool_use_id FROM pending_tool_calls", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(remaining, "toolu_recent");
    }
}
