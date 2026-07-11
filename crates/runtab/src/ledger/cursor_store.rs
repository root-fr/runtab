use rusqlite::{params, OptionalExtension};

use super::Ledger;
use crate::adapters::SourceCursorState;

impl Ledger {
    /// The stored cursor for a DB-backed source, or `None` on the first scan.
    pub fn source_cursor(&self, source: &str) -> rusqlite::Result<Option<SourceCursorState>> {
        self.conn
            .query_row(
                "SELECT db_path, cursor, row_count FROM source_cursors WHERE source = ?1",
                params![source],
                |r| {
                    Ok(SourceCursorState {
                        db_path: r.get(0)?,
                        cursor: r.get(1)?,
                        row_count: r.get(2)?,
                    })
                },
            )
            .optional()
    }

    /// Persist a source's post-scan cursor. Upserts on the `source` key so a
    /// re-scan overwrites the prior watermark.
    pub fn set_source_cursor(
        &self,
        source: &str,
        db_path: &str,
        cursor: &str,
        row_count: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO source_cursors (source, db_path, cursor, row_count, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source) DO UPDATE SET
                db_path = excluded.db_path, cursor = excluded.cursor,
                row_count = excluded.row_count, updated_at = excluded.updated_at",
            params![source, db_path, cursor, row_count, crate::timeutil::now_rfc3339()],
        )?;
        Ok(())
    }

    /// Clear the re-push `dirty` flag on the given rows, once the server has
    /// accepted (or already held) them. Excluded rows are cleared too so their
    /// flag does not force a re-scan every batch.
    pub fn clear_dirty(&self, ids: &[i64]) -> rusqlite::Result<()> {
        for id in ids {
            self.conn
                .execute("UPDATE usage_events SET dirty = 0 WHERE id = ?1", params![id])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Ledger;

    #[test]
    fn source_cursor_roundtrips() {
        let l = Ledger::open_in_memory().unwrap();
        assert!(l.source_cursor("opencode").unwrap().is_none());

        l.set_source_cursor("opencode", "/db/opencode.db", "{\"tu\":123}", 7).unwrap();
        let s = l.source_cursor("opencode").unwrap().unwrap();
        assert_eq!(s.db_path, "/db/opencode.db");
        assert_eq!(s.cursor, "{\"tu\":123}");
        assert_eq!(s.row_count, 7);
    }

    #[test]
    fn set_source_cursor_upserts_on_conflict() {
        let l = Ledger::open_in_memory().unwrap();
        l.set_source_cursor("hermes", "/a.db", "c1", 1).unwrap();
        l.set_source_cursor("hermes", "/b.db", "c2", 42).unwrap();

        let s = l.source_cursor("hermes").unwrap().unwrap();
        assert_eq!(s.db_path, "/b.db");
        assert_eq!(s.cursor, "c2");
        assert_eq!(s.row_count, 42);
    }

    #[test]
    fn source_cursors_are_isolated_per_source() {
        let l = Ledger::open_in_memory().unwrap();
        l.set_source_cursor("opencode", "/oc.db", "oc", 3).unwrap();
        l.set_source_cursor("hermes", "/h.db", "hm", 9).unwrap();

        assert_eq!(l.source_cursor("opencode").unwrap().unwrap().cursor, "oc");
        assert_eq!(l.source_cursor("hermes").unwrap().unwrap().cursor, "hm");
    }

    #[test]
    fn clear_dirty_resets_the_flag() {
        let l = Ledger::open_in_memory().unwrap();
        l.conn
            .execute(
                "INSERT INTO usage_events
                    (source, message_id, request_id, session_id, ts, model,
                     input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                     cache_1h_tokens, cache_5m_tokens, reasoning_tokens,
                     project, agent_version, cost_usd, cost_basis, dirty)
                 VALUES ('hermes','m','','s','2026-07-01T10:00:00Z','x',
                         1,0,0,0,0,0,0,'p','',NULL,'estimated',1)",
                [],
            )
            .unwrap();
        let id: i64 = l
            .conn
            .query_row("SELECT id FROM usage_events", [], |r| r.get(0))
            .unwrap();
        let dirty: i64 = l
            .conn
            .query_row("SELECT dirty FROM usage_events WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(dirty, 1);

        l.clear_dirty(&[id]).unwrap();
        let dirty: i64 = l
            .conn
            .query_row("SELECT dirty FROM usage_events WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(dirty, 0);
    }
}
