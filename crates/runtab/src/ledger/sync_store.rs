use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::Ledger;
use crate::billing::{self, Mode};
use crate::wire::PulledRecord;

#[derive(Debug, Clone, Serialize)]
pub struct SyncState {
    pub enabled: bool,
    pub account_email: Option<String>,
    pub user_id: Option<String>,
    pub server_url: Option<String>,
    pub pull_cursor: i64,
    pub last_pushed_id: i64,
    pub last_push_at: Option<String>,
    pub last_pull_at: Option<String>,
    pub degraded: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub billing_mode_override: Option<String>,
    pub plan_limit_5h_tokens: u64,
    pub plan_limit_weekly_tokens: u64,
    #[serde(default)]
    pub machine_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MachineStat {
    pub machine_id: String,
    pub machine_name: String,
    pub is_current: bool,
    pub last_seen: Option<String>,
    pub event_count: u64,
}

impl Ledger {
    pub(crate) fn override_mode(&self) -> rusqlite::Result<Option<Mode>> {
        let raw: Option<String> = self.conn.query_row(
            "SELECT billing_mode_override FROM app_settings WHERE id = 1",
            [],
            |r| r.get(0),
        )?;
        Ok(billing::parse_override(raw.as_deref()))
    }

    pub(crate) fn plan_limits(&self) -> rusqlite::Result<(i64, i64)> {
        self.conn.query_row(
            "SELECT plan_limit_5h_tokens, plan_limit_weekly_tokens FROM app_settings WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    pub fn settings(&self) -> rusqlite::Result<Settings> {
        let (ov, l5, lw): (Option<String>, i64, i64) = self.conn.query_row(
            "SELECT billing_mode_override, plan_limit_5h_tokens, plan_limit_weekly_tokens
             FROM app_settings WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        // Read the machine name from the persisted row, not the cached identity,
        // so a rename via `PUT /api/settings` is reflected immediately.
        let machine_name: String =
            self.conn
                .query_row("SELECT machine_name FROM sync_state WHERE id = 1", [], |r| r.get(0))?;
        Ok(Settings {
            billing_mode_override: ov,
            plan_limit_5h_tokens: l5.max(0) as u64,
            plan_limit_weekly_tokens: lw.max(0) as u64,
            machine_name,
        })
    }

    /// Persist settings. An override outside the known set is dropped to null
    /// (auto), never stored, so the value can only ever be a valid mode.
    pub fn update_settings(&self, s: &Settings) -> rusqlite::Result<Settings> {
        let ov = match s.billing_mode_override.as_deref() {
            Some("subscription") => Some("subscription"),
            Some("api") => Some("api"),
            _ => None,
        };
        self.conn.execute(
            "UPDATE app_settings SET billing_mode_override = ?1,
                 plan_limit_5h_tokens = ?2, plan_limit_weekly_tokens = ?3 WHERE id = 1",
            params![ov, s.plan_limit_5h_tokens as i64, s.plan_limit_weekly_tokens as i64],
        )?;
        let name = s.machine_name.trim();
        if !name.is_empty() {
            // Clamp to the server's machine_name cap so a rename can never stamp a
            // value that would later 422 and wedge every batch (contract §4).
            self.conn.execute(
                "UPDATE sync_state SET machine_name = ?1 WHERE id = 1",
                params![super::clamp_chars(name, 64)],
            )?;
        }
        self.settings()
    }

    pub fn sync_state(&self) -> rusqlite::Result<SyncState> {
        self.conn.query_row(
            "SELECT enabled, account_email, user_id, server_url, pull_cursor,
                    last_pushed_id, last_push_at, last_pull_at, degraded, message
             FROM sync_state WHERE id = 1",
            [],
            |r| {
                Ok(SyncState {
                    enabled: r.get::<_, i64>(0)? != 0,
                    account_email: r.get(1)?,
                    user_id: r.get(2)?,
                    server_url: r.get(3)?,
                    pull_cursor: r.get(4)?,
                    last_pushed_id: r.get(5)?,
                    last_push_at: r.get(6)?,
                    last_pull_at: r.get(7)?,
                    degraded: r.get::<_, i64>(8)? != 0,
                    message: r.get(9)?,
                })
            },
        )
    }

    pub fn enable_sync(&self, email: &str, user_id: &str, server_url: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE sync_state SET enabled = 1, account_email = ?1, user_id = ?2,
                 server_url = ?3, degraded = 0, message = NULL WHERE id = 1",
            params![email, user_id, server_url],
        )?;
        Ok(())
    }

    pub fn disable_sync(&self) -> rusqlite::Result<()> {
        self.conn
            .execute("UPDATE sync_state SET enabled = 0, degraded = 0, message = NULL WHERE id = 1", [])?;
        Ok(())
    }

    /// Clear all synced state after a server account wipe. The local ledger of
    /// this machine's own rows is kept (it is authoritative and re-pushes
    /// idempotently), but pulled rows and cursors are dropped so a later
    /// re-login starts clean.
    pub fn reset_sync(&self) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM remote_events", [])?;
        self.conn.execute("DELETE FROM project_prefs", [])?;
        self.conn.execute(
            "UPDATE sync_state SET enabled = 0, account_email = NULL, user_id = NULL,
                 pull_cursor = 0, last_pushed_id = 0, degraded = 0, message = NULL,
                 projects_reviewed = 0 WHERE id = 1",
            [],
        )?;
        Ok(())
    }

    pub fn set_degraded(&self, degraded: bool, message: Option<&str>) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE sync_state SET degraded = ?1, message = ?2 WHERE id = 1",
            params![degraded as i64, message],
        )?;
        Ok(())
    }

    /// Count of not-yet-pushed rows that would actually be sent — excluded
    /// projects don't count, so the status pill never shows a phantom backlog for
    /// projects the user chose to keep local.
    pub fn pending_push_count(&self) -> rusqlite::Result<i64> {
        let last: i64 =
            self.conn
                .query_row("SELECT last_pushed_id FROM sync_state WHERE id = 1", [], |r| r.get(0))?;
        self.conn.query_row(
            "SELECT COUNT(*) FROM usage_events ue
             LEFT JOIN project_prefs pp ON pp.name = COALESCE(ue.project_label, ue.project)
             WHERE ue.id > ?1 AND COALESCE(pp.excluded, 0) = 0",
            params![last],
            |r| r.get(0),
        )
    }

    pub fn set_last_pushed_id(&self, id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE sync_state SET last_pushed_id = ?1, last_push_at = ?2 WHERE id = 1",
            params![id, crate::timeutil::now_rfc3339()],
        )?;
        Ok(())
    }

    /// Idempotently store a pulled row from another machine. The `event_id`
    /// UNIQUE constraint drops re-pulled duplicates.
    pub fn upsert_remote(&self, p: &PulledRecord) -> rusqlite::Result<()> {
        let r = &p.record;
        self.conn.execute(
            "INSERT OR IGNORE INTO remote_events
                (server_seq, event_id, ts, agent, model, project_label, session_id,
                 machine_id, machine_name, input_tokens, output_tokens, cache_read_tokens,
                 cache_creation_tokens, reasoning_tokens, est_cost_microusd, cost_basis)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                p.server_seq, r.event_id, r.ts, r.agent, r.model, r.project_label, r.session_id,
                r.machine_id, r.machine_name, r.input_tokens as i64, r.output_tokens as i64,
                r.cache_read_tokens as i64, r.cache_creation_tokens as i64, r.reasoning_tokens as i64,
                r.est_cost_microusd as i64, r.cost_basis
            ],
        )?;
        Ok(())
    }

    pub fn set_pull_cursor(&self, cursor: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE sync_state SET pull_cursor = ?1, last_pull_at = ?2 WHERE id = 1",
            params![cursor, crate::timeutil::now_rfc3339()],
        )?;
        Ok(())
    }

    pub fn machine_stats(&self) -> rusqlite::Result<Vec<MachineStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT machine_id, machine_name, MAX(ts), COUNT(*) FROM merged_events
             WHERE machine_id IS NOT NULL GROUP BY machine_id ORDER BY COUNT(*) DESC",
        )?;
        let cur = self.machine_id.clone();
        let rows = stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            Ok(MachineStat {
                is_current: id == cur,
                machine_id: id,
                machine_name: r.get(1)?,
                last_seen: r.get(2)?,
                event_count: uu(r.get(3)?),
            })
        })?;
        rows.collect()
    }
}

fn uu(v: i64) -> u64 {
    v.max(0) as u64
}
