use rusqlite::{params, OptionalExtension};

use super::Ledger;
use crate::encoding::new_uuid;

impl Ledger {
    /// Establish the stable machine identity and backfill rows that predate the
    /// machine dimension. Idempotent: the identity row and settings row are
    /// created once, then reused.
    pub(super) fn ensure_identity(&self) -> rusqlite::Result<(String, String)> {
        let existing: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT machine_id, machine_name FROM sync_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        let (id, name) = match existing {
            Some(v) => v,
            None => {
                let id = new_uuid();
                let name = default_machine_name();
                self.conn.execute(
                    "INSERT INTO sync_state (id, machine_id, machine_name) VALUES (1, ?1, ?2)",
                    params![id, name],
                )?;
                (id, name)
            }
        };
        self.conn
            .execute("INSERT OR IGNORE INTO app_settings (id) VALUES (1)", [])?;
        self.backfill(&id, &name)?;
        Ok((id, name))
    }

    /// Stamp the machine dimension and a `project_label` onto rows migrated from
    /// a pre-sync database. New rows are stamped at insert time instead. Runs as
    /// one transaction: the per-row label loop in autocommit would pay one WAL
    /// commit per row (minutes on a large ledger).
    fn backfill(&self, machine_id: &str, machine_name: &str) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match self.backfill_inner(machine_id, machine_name) {
            Ok(()) => self.conn.execute_batch("COMMIT"),
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    fn backfill_inner(&self, machine_id: &str, machine_name: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE usage_events SET machine_id = ?1 WHERE machine_id IS NULL",
            params![machine_id],
        )?;
        self.conn.execute(
            "UPDATE usage_events SET machine_name = ?1 WHERE machine_name IS NULL",
            params![machine_name],
        )?;

        let mut stmt = self
            .conn
            .prepare("SELECT id, project FROM usage_events WHERE project_label IS NULL")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        for (id, project) in rows {
            self.conn.execute(
                "UPDATE usage_events SET project_label = ?1 WHERE id = ?2",
                params![basename(&project), id],
            )?;
        }
        Ok(())
    }
}

/// The synced, path-free project label: the trailing path component of the full
/// local `cwd`. Full paths never leave the machine (spec §13).
pub fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let cut = trimmed.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    let name = &trimmed[cut..];
    if name.is_empty() {
        path.to_string()
    } else {
        name.to_string()
    }
}

/// Truncate to at most `max` characters on a UTF-8 boundary (never mid-codepoint).
/// Keeps synced fields within the server's fail-closed length caps so one
/// over-long label can't 422 and wedge the whole append-only push.
pub fn clamp_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => s[..idx].to_string(),
        None => s.to_string(),
    }
}

/// Default machine label: explicit `RUNTAB_MACHINE_NAME` (docker sets a distinct
/// one per machine), else the OS hostname, else a stable placeholder.
fn default_machine_name() -> String {
    let raw = if let Some(v) = std::env::var_os("RUNTAB_MACHINE_NAME") {
        let s = v.to_string_lossy().trim().to_string();
        if s.is_empty() { hostname() } else { Some(s) }
    } else {
        hostname()
    };
    // Clamp to the server's 64-char machine_name cap at the source.
    clamp_chars(&raw.unwrap_or_else(|| "runtab-machine".to_string()), 64)
}

fn hostname() -> Option<String> {
    if let Some(v) = std::env::var_os("HOSTNAME") {
        let s = v.to_string_lossy().trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
