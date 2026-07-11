use rusqlite::params;

use super::{basename, clamp_chars, Ledger};
use crate::encoding::sha256_hex;
use crate::wire::SyncedRecord;

// Server-enforced caps (contract §Synced record + hardening addendum §4).
// Mirrored client-side so a locally valid but over-long value is truncated
// before it can 422 the whole append-only batch and wedge sync permanently.
const MAX_PROJECT_LABEL: usize = 120;
const MAX_MACHINE_NAME: usize = 64;
const MAX_MODEL: usize = 100;

// Server validate.rs caps, mirrored: a cumulative counter (hermes) can plausibly
// cross these, and a 422 rejects the whole append-only batch permanently. The
// local ledger stays exact — only the wire copy saturates.
const MAX_TOKENS: u64 = 100_000_000;
const MAX_COST: u64 = 10_000_000_000;

// Row column order shared by `pending_batch` and `preview_record`.
const ROW_COLUMNS: &str = "ue.id, ue.source, ue.message_id, ue.request_id, ue.session_id, ue.ts,
    ue.model, ue.project_label, ue.project, pp.synced_label, COALESCE(pp.excluded, 0),
    ue.machine_id, ue.machine_name, ue.input_tokens, ue.output_tokens, ue.cache_read_tokens,
    ue.cache_creation_tokens, ue.reasoning_tokens, ue.cost_usd, ue.cost_basis, ue.dirty";

/// A batch of local rows turned into whitelist records, plus the highest row id
/// scanned (so the cursor advances past excluded rows) and how many rows were
/// scanned (so the caller knows whether more remain).
pub struct PendingBatch {
    pub records: Vec<SyncedRecord>,
    pub max_id: i64,
    pub scanned: usize,
    /// Ids of every scanned row carrying `dirty = 1`, including excluded ones.
    /// The caller clears these after the server accepts the batch so a grown
    /// row is re-pushed exactly once per growth (excluded rows never push, so
    /// their flag must still be cleared or they re-scan every batch forever).
    pub dirty_ids: Vec<i64>,
}

impl Ledger {
    /// The next batch of unpushed rows above the cursor, as fail-closed whitelist
    /// records. Returns nothing until the projects have been reviewed (the consent
    /// gate), so no label leaves the machine before the user has seen it. Excluded
    /// projects are skipped; renamed projects use the chosen label; every field is
    /// clamped to the server's caps; and a NULL `project_label` falls back to
    /// `basename(project)`, never the raw `cwd`.
    pub fn pending_batch(&self, limit: i64) -> rusqlite::Result<PendingBatch> {
        let last: i64 = self.conn.query_row(
            "SELECT last_pushed_id FROM sync_state WHERE id = 1",
            [],
            |r| r.get(0),
        )?;
        if !self.projects_reviewed()? {
            return Ok(PendingBatch { records: Vec::new(), max_id: last, scanned: 0, dirty_ids: Vec::new() });
        }
        // `id > cursor` catches new rows; `dirty = 1` re-catches grown rows below
        // the cursor (the append-only id watermark never re-selects an in-place
        // update).
        let sql = format!(
            "SELECT {ROW_COLUMNS} FROM usage_events ue
             LEFT JOIN project_prefs pp ON pp.name = COALESCE(ue.project_label, ue.project)
             WHERE (ue.id > ?1 OR ue.dirty = 1) ORDER BY ue.id ASC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut max_id = last;
        let mut scanned = 0usize;
        let mut records = Vec::new();
        let mut dirty_ids = Vec::new();
        let mut rows = stmt.query(params![last, limit])?;
        while let Some(r) = rows.next()? {
            let id = r.get::<_, i64>(0)?;
            max_id = max_id.max(id);
            scanned += 1;
            if r.get::<_, i64>(20)? != 0 {
                dirty_ids.push(id);
            }
            if r.get::<_, i64>(10)? != 0 {
                continue;
            }
            records.push(self.row_to_record(r)?);
        }
        Ok(PendingBatch { records, max_id, scanned, dirty_ids })
    }

    /// One real derived record for the "See exactly what syncs" drawer: the exact
    /// shape and values that would be uploaded, built from the earliest local row.
    /// Deliberately not gated on review — the point is to show it *before* the user
    /// enables sync. `None` when the ledger is empty.
    pub fn preview_record(&self) -> rusqlite::Result<Option<SyncedRecord>> {
        let sql = format!(
            "SELECT {ROW_COLUMNS} FROM usage_events ue
             LEFT JOIN project_prefs pp ON pp.name = COALESCE(ue.project_label, ue.project)
             ORDER BY ue.id ASC LIMIT 1"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        match rows.next()? {
            Some(r) => Ok(Some(self.row_to_record(r)?)),
            None => Ok(None),
        }
    }

    fn row_to_record(&self, r: &rusqlite::Row) -> rusqlite::Result<SyncedRecord> {
        let source: String = r.get(1)?;
        let message_id: String = r.get(2)?;
        let request_id: String = r.get(3)?;
        let session_id: String = r.get(4)?;
        let project_label: Option<String> = r.get(7)?;
        let project: String = r.get(8)?;
        let synced_label: Option<String> = r.get(9)?;
        let label = synced_label.or(project_label).unwrap_or_else(|| basename(&project));
        let machine_name = r
            .get::<_, Option<String>>(12)?
            .unwrap_or_else(|| self.machine_name.clone());
        let output = uu(r.get(14)?);
        let reasoning = uu(r.get(17)?).min(output);
        let cost_usd: Option<f64> = r.get(18)?;
        Ok(SyncedRecord {
            event_id: sha256_hex(&[&source, &message_id, &request_id]),
            ts: r.get(5)?,
            agent: source.replace('_', "-"),
            model: clamp_chars(&r.get::<_, String>(6)?, MAX_MODEL),
            project_label: clamp_chars(&label, MAX_PROJECT_LABEL),
            session_id: sha256_hex(&[&session_id]),
            machine_id: r.get::<_, Option<String>>(11)?.unwrap_or_else(|| self.machine_id.clone()),
            machine_name: clamp_chars(&machine_name, MAX_MACHINE_NAME),
            input_tokens: uu(r.get(13)?),
            output_tokens: output,
            cache_read_tokens: uu(r.get(15)?),
            cache_creation_tokens: uu(r.get(16)?),
            reasoning_tokens: reasoning,
            est_cost_microusd: micro(cost_usd),
            cost_basis: r.get(19)?,
        })
    }
}

fn uu(v: i64) -> u64 {
    (v.max(0) as u64).min(MAX_TOKENS)
}

fn micro(cost: Option<f64>) -> u64 {
    match cost {
        Some(c) if c > 0.0 => ((c * 1_000_000.0).round() as u64).min(MAX_COST),
        _ => 0,
    }
}
