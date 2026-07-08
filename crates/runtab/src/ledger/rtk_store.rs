//! `rtk_events` / `rtk_scan_state` access for the rtk importer (see
//! `crate::rtkimport`). Kept next to `tool_store.rs` for the same reason:
//! these are small, table-specific helpers on `Ledger`.

use rusqlite::{params, OptionalExtension};

use super::Ledger;
use crate::model::{RtkCommandRow, UnattributedRtkRow};

impl Ledger {
    /// `(db_path, last_row_id)` of the last completed import, or `None` if
    /// rtk history has never been imported.
    pub fn rtk_scan_cursor(&self) -> rusqlite::Result<Option<(String, i64)>> {
        self.conn
            .query_row(
                "SELECT db_path, last_row_id FROM rtk_scan_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    /// Only touches `db_path`/`last_row_id`: on conflict, the `UPDATE` arm
    /// leaves `last_attributed_rtk_id` exactly as it was, so an import never
    /// clobbers the attribution watermark (and vice versa — see
    /// `set_attribution_watermark`).
    pub fn set_rtk_scan_cursor(&self, db_path: &str, last_row_id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO rtk_scan_state (id, db_path, last_row_id) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET db_path = excluded.db_path, last_row_id = excluded.last_row_id",
            params![db_path, last_row_id],
        )?;
        Ok(())
    }

    /// How far `attribute` has examined `rtk_events` by internal `id` (not
    /// rtk's own `rtk_row_id`, and not reset when rtk's history db resets —
    /// see the comment on `import`'s reset detection). `0` when the row
    /// doesn't exist yet (fresh ledger, or `attribute` has never run).
    pub fn attribution_watermark(&self) -> rusqlite::Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT last_attributed_rtk_id FROM rtk_scan_state WHERE id = 1", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0))
    }

    /// Only touches `last_attributed_rtk_id`: on conflict, the `UPDATE` arm
    /// leaves `db_path`/`last_row_id` exactly as they were. The `db_path`/
    /// `last_row_id` literals in the `INSERT` arm only apply when no row
    /// exists yet (e.g. `attribute` runs before any import ever has); they
    /// get overwritten by the next `set_rtk_scan_cursor` call.
    ///
    /// The `UPDATE` keeps `MAX(current, incoming)` rather than overwriting:
    /// `attribute` fetches unattributed rows in `id` order but processes them
    /// in chunks, and the retry-horizon clause (`ts > cutoff`) can put
    /// low-id rows in a later chunk than a high-id row already committed
    /// earlier. A crash between chunks must not let a low-id chunk's commit
    /// move the watermark backwards — that would just force a wasteful
    /// re-scan of old `none` rows on the next run, not corrupt anything, but
    /// there's no reason to pay for it.
    pub fn set_attribution_watermark(&self, last_attributed_rtk_id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO rtk_scan_state (id, db_path, last_row_id, last_attributed_rtk_id)
             VALUES (1, '', 0, ?1)
             ON CONFLICT(id) DO UPDATE SET
                last_attributed_rtk_id = MAX(last_attributed_rtk_id, excluded.last_attributed_rtk_id)",
            params![last_attributed_rtk_id],
        )?;
        Ok(())
    }

    /// Insert one imported rtk command. `INSERT OR IGNORE` on the
    /// `rtk_row_id` UNIQUE constraint absorbs re-imports after a cursor
    /// restart (see `rtkimport::import`) — the older row wins, the replay is
    /// silently dropped. Returns whether a new row actually landed.
    pub fn insert_rtk_event(&self, row: &RtkCommandRow) -> rusqlite::Result<bool> {
        self.conn.execute(
            "INSERT OR IGNORE INTO rtk_events
                (rtk_row_id, ts, project_path, head_hash, cmd_hash,
                 raw_tokens, filtered_tokens, saved_tokens, exec_time_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                row.rtk_row_id,
                row.ts,
                row.project_path,
                row.head_hash,
                row.cmd_hash,
                row.raw_tokens,
                row.filtered_tokens,
                row.saved_tokens,
                row.exec_time_ms,
            ],
        )?;
        Ok(self.conn.changes() > 0)
    }

    /// `rtk_events` rows still needing attribution: either never examined by
    /// a previous `attribute` run (`id > watermark`, which is how an
    /// interrupted backfill resumes instead of leaving permanent gaps) or
    /// recent enough that a late-arriving transcript might still explain
    /// them (`ts > retry_horizon_cutoff`). Ordered by `id` ascending so the
    /// caller can advance the watermark chunk-by-chunk using each chunk's
    /// last row.
    pub fn fetch_unattributed_rtk_rows(
        &self,
        watermark: i64,
        retry_horizon_cutoff: &str,
    ) -> rusqlite::Result<Vec<UnattributedRtkRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, project_path, head_hash, cmd_hash
             FROM rtk_events
             WHERE match_kind = 'none'
               AND (id > ?1 OR ts > ?2)
             ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![watermark, retry_horizon_cutoff], |r| {
                Ok(UnattributedRtkRow {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    project_path: r.get(2)?,
                    head_hash: r.get(3)?,
                    cmd_hash: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Record a match on rtk row `id` (our internal `rtk_events.id`, not
    /// rtk's own `rtk_row_id`).
    pub fn update_rtk_attribution(
        &self,
        id: i64,
        source: &str,
        session_id: &str,
        tool_event_id: i64,
        match_kind: &str,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE rtk_events SET source = ?1, session_id = ?2, tool_event_id = ?3, match_kind = ?4
             WHERE id = ?5",
            params![source, session_id, tool_event_id, match_kind, id],
        )?;
        Ok(())
    }
}

/// `(id, source, session_id, tool_event_id, match_kind)`.
#[cfg(test)]
type RtkEventState = (i64, Option<String>, Option<String>, Option<i64>, String);

#[cfg(test)]
impl Ledger {
    /// Test helper: full attribution state of one imported rtk row, looked
    /// up by rtk's own `rtk_row_id` (not our internal `id`). See
    /// `RtkEventState` for the field order.
    pub fn rtk_event_state(&self, rtk_row_id: i64) -> rusqlite::Result<Option<RtkEventState>> {
        self.conn
            .query_row(
                "SELECT id, source, session_id, tool_event_id, match_kind
                 FROM rtk_events WHERE rtk_row_id = ?1",
                params![rtk_row_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watermark_never_moves_backwards() {
        let l = Ledger::open_in_memory().unwrap();

        l.set_attribution_watermark(100).unwrap();
        assert_eq!(l.attribution_watermark().unwrap(), 100);

        // A lower watermark (e.g. an earlier retry-horizon chunk committing
        // after a later id-ordered chunk already advanced it) must not move
        // the persisted value backwards.
        l.set_attribution_watermark(50).unwrap();
        assert_eq!(l.attribution_watermark().unwrap(), 100);

        l.set_attribution_watermark(150).unwrap();
        assert_eq!(l.attribution_watermark().unwrap(), 150);
    }
}
