mod api_daily;
mod api_query;
mod api_sessions;
mod identity;
mod project_prefs;
mod push_rows;
mod query;
mod rtk_store;
mod savings_query;
mod schema;
mod sync_store;
mod tool_query;
mod tool_store;

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::UsageEvent;

pub use api_daily::{DailyDay, DailyModel, HeatmapCell};
pub use api_query::{Filter, ModeBreak, ModelRow, ProjectRow, Summary};
pub use api_sessions::{PlanUsage, PlanWindows, SessionPage, SessionRow, Window};
pub use identity::{basename, clamp_chars};
pub use project_prefs::ReviewItem;
pub use push_rows::PendingBatch;
pub use query::AggregateRow;
pub use savings_query::{SavingsDaily, SavingsReport, SavingsWindow};
pub use sync_store::{MachineStat, Settings, SyncState};
pub use tool_query::{RtkTotals, ToolAggregateRow};

/// Incremental scan state for one transcript file. JSONL is append-only, so a
/// stored `byte_offset` lets re-scans read only the bytes added since last time.
pub struct FileState {
    pub size: u64,
    pub mtime: i64,
    pub byte_offset: u64,
}

/// Outcome of an upsert under the `UNIQUE(source, message_id, request_id)`
/// dedup key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertResult {
    Inserted,
    ReplacedHigher,
    KeptExisting,
}

pub struct Ledger {
    conn: Connection,
    machine_id: String,
    machine_name: String,
}

impl Ledger {
    pub fn open(path: &Path) -> anyhow::Result<Ledger> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Ledger> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> anyhow::Result<Ledger> {
        // Tolerate a concurrent writer (e.g. a manual scan racing a Stop hook, or
        // the `serve` daemon sharing the file with a `sync` CLI) instead of
        // failing an upsert on the default 0ms SQLITE_BUSY. WAL lets a reader
        // (dashboard query) and a writer (scan/sync) proceed without blocking;
        // it is a no-op for in-memory test databases.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        // NORMAL under WAL trades at-most-the-last-commit durability on power
        // loss for one fsync per checkpoint instead of per commit. The ledger is
        // rebuildable from the source logs, and FULL turns a large backfill into
        // an fsync storm (minutes on CoW filesystems like btrfs).
        conn.execute_batch("PRAGMA synchronous = NORMAL")?;
        schema::create(&conn)?;
        let mut ledger = Ledger {
            conn,
            machine_id: String::new(),
            machine_name: String::new(),
        };
        let (id, name) = ledger.ensure_identity()?;
        ledger.machine_id = id;
        ledger.machine_name = name;
        Ok(ledger)
    }

    /// Batch bulk upserts into one WAL commit. Per-event autocommit pays one
    /// commit per row, which turns a 100k-event backfill into minutes.
    pub fn tx_begin(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE")
    }

    pub fn tx_commit(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch("COMMIT")
    }

    pub fn tx_rollback(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch("ROLLBACK")
    }

    pub fn machine_id(&self) -> &str {
        &self.machine_id
    }

    pub fn machine_name(&self) -> &str {
        &self.machine_name
    }

    /// Insert, or on the dedup key keep whichever row has the higher total token
    /// count (handles Claude streaming-replay duplicates).
    pub fn upsert(&self, e: &UsageEvent) -> rusqlite::Result<UpsertResult> {
        let total_new = e.total_tokens();
        let sql = format!(
            "SELECT id, {total}
             FROM usage_events
             WHERE source = ?1 AND message_id = ?2 AND request_id = ?3",
            total = schema::TOTAL_TOKENS_EXPR
        );
        let existing: Option<(i64, i64)> = self
            .conn
            .query_row(
                &sql,
                params![e.source, e.message_id, e.request_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        match existing {
            None => {
                self.insert_row(e)?;
                Ok(UpsertResult::Inserted)
            }
            Some((id, total_old)) => {
                if total_new > total_old {
                    self.update_row(id, e)?;
                    Ok(UpsertResult::ReplacedHigher)
                } else {
                    Ok(UpsertResult::KeptExisting)
                }
            }
        }
    }

    fn insert_row(&self, e: &UsageEvent) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO usage_events
                (source, message_id, request_id, session_id, ts, model,
                 input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                 cache_1h_tokens, cache_5m_tokens, reasoning_tokens,
                 project, agent_version, cost_usd, cost_basis,
                 project_label, machine_id, machine_name)
             VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19, ?20)",
            params![
                e.source, e.message_id, e.request_id, e.session_id, e.ts, e.model,
                e.input_tokens, e.output_tokens, e.cache_read_tokens, e.cache_creation_tokens,
                e.cache_1h_tokens, e.cache_5m_tokens, e.reasoning_tokens,
                e.project, e.agent_version, e.cost_usd, e.cost_basis.as_str(),
                basename(&e.project), self.machine_id, self.machine_name
            ],
        )?;
        Ok(())
    }

    fn update_row(&self, id: i64, e: &UsageEvent) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE usage_events SET
                session_id = ?1, ts = ?2, model = ?3,
                input_tokens = ?4, output_tokens = ?5, cache_read_tokens = ?6,
                cache_creation_tokens = ?7, cache_1h_tokens = ?8, cache_5m_tokens = ?9,
                reasoning_tokens = ?10, project = ?11, agent_version = ?12,
                cost_usd = ?13, cost_basis = ?14, project_label = ?16
             WHERE id = ?15",
            params![
                e.session_id, e.ts, e.model,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_creation_tokens, e.cache_1h_tokens, e.cache_5m_tokens,
                e.reasoning_tokens, e.project, e.agent_version,
                e.cost_usd, e.cost_basis.as_str(), id, basename(&e.project)
            ],
        )?;
        Ok(())
    }

    pub fn file_state(&self, path: &Path) -> rusqlite::Result<Option<FileState>> {
        self.conn
            .query_row(
                "SELECT size, mtime, byte_offset FROM scanned_files WHERE path = ?1",
                params![path.to_string_lossy()],
                |r| {
                    Ok(FileState {
                        size: r.get::<_, i64>(0)? as u64,
                        mtime: r.get(1)?,
                        byte_offset: r.get::<_, i64>(2)? as u64,
                    })
                },
            )
            .optional()
    }

    pub fn set_file_state(
        &self,
        path: &Path,
        size: u64,
        mtime: i64,
        byte_offset: u64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO scanned_files (path, size, mtime, byte_offset)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET
                size = excluded.size, mtime = excluded.mtime, byte_offset = excluded.byte_offset",
            params![path.to_string_lossy(), size as i64, mtime, byte_offset as i64],
        )?;
        Ok(())
    }
}

/// Default DB location: `$XDG_DATA_HOME/runtab/runtab.db`, falling back to
/// `~/.local/share/runtab/runtab.db`.
pub fn default_db_path() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| crate::home_dir().map(|h| h.join(".local").join("share")))
        .ok_or_else(|| anyhow::anyhow!("cannot determine data directory (set XDG_DATA_HOME or HOME)"))?;
    Ok(base.join("runtab").join("runtab.db"))
}
