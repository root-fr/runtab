use std::io;
use std::path::{Path, PathBuf};

use crate::model::{ToolResultSeen, ToolUseSeen, UsageEvent};

mod claude_code;
mod claude_tools;
mod codex;

pub use claude_code::ClaudeCodeAdapter;
pub use codex::CodexAdapter;

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
