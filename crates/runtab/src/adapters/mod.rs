use std::io;
use std::path::{Path, PathBuf};

use crate::model::{ToolResultSeen, ToolUseSeen, UsageEvent};

mod claude_code;
mod claude_tools;
mod codex;
mod hermes;
mod opencode;

pub use claude_code::ClaudeCodeAdapter;
pub use codex::{codex_discovery_roots_from, CodexAdapter};
pub use hermes::HermesAdapter;
pub use opencode::OpencodeAdapter;

/// Result of parsing one transcript file from a byte offset to EOF.
pub struct ParseOutput {
    pub events: Vec<UsageEvent>,
    /// `tool_use` blocks seen (may be missing their result if the transcript
    /// was only partially written; pairing happens ledger-side).
    pub tool_uses: Vec<ToolUseSeen>,
    /// `tool_result` blocks seen.
    pub tool_results: Vec<ToolResultSeen>,
    /// Looked like a usage or tool record (widened substring pre-filter) but
    /// failed to parse or validate.
    pub lines_skipped: u64,
    /// Byte offset to resume from on the next scan.
    pub new_offset: u64,
}

/// A source of coding-agent usage logs.
pub trait Adapter {
    /// Discover transcript files this adapter can read.
    fn discover(&self) -> Vec<PathBuf>;

    /// Parse a file starting at `byte_offset`. I/O errors are returned so the
    /// caller can log and skip the file; malformed lines are counted, not fatal.
    fn parse_from(&self, path: &Path, byte_offset: u64) -> io::Result<ParseOutput>;
}

/// State stored per DB-backed source in `source_cursors` (migration v4).
pub struct SourceCursorState {
    pub db_path: String,
    /// Opaque, adapter-owned encoding ("" = full-scan-every-time source).
    pub cursor: String,
    /// Upstream row count at last scan, for reset detection.
    pub row_count: i64,
}

/// One incremental read from a DB-backed source.
pub struct DbFetch {
    pub events: Vec<UsageEvent>,
    /// Rows that looked usage-shaped but failed to parse/validate.
    pub rows_skipped: u64,
    pub new_cursor: String,
    pub row_count: i64,
}

/// A usage source backed by another tool's SQLite database. Opened strictly
/// read-only; absence, SQLITE_BUSY, and schema drift are per-tick skips,
/// never scan failures.
pub trait DbAdapter {
    /// Stable source string written into `usage_events.source` and used as the
    /// `source_cursors` primary key. Must survive the wire mapping
    /// `source.replace('_', "-")` in `push_rows.rs`.
    fn source(&self) -> &'static str;
    /// Locate the source DB. None = not installed (feature silently off).
    fn discover(&self) -> Option<PathBuf>;
    /// Read past the stored cursor; the adapter owns reset detection.
    fn fetch(&self, db_path: &Path, stored: Option<&SourceCursorState>) -> anyhow::Result<DbFetch>;
}
