use std::io;
use std::path::{Path, PathBuf};

use super::{Adapter, ParseOutput};

/// Codex CLI adapter. Stub for v0: discovers nothing and parses nothing. Wired
/// into the CLI so the cross-agent surface exists; real cumulative-token-diff
/// parsing lands later.
#[derive(Default)]
pub struct CodexAdapter;

impl Adapter for CodexAdapter {
    fn discover(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn parse_from(&self, _path: &Path, byte_offset: u64) -> io::Result<ParseOutput> {
        Ok(ParseOutput {
            events: Vec::new(),
            tool_uses: Vec::new(),
            tool_results: Vec::new(),
            lines_skipped: 0,
            new_offset: byte_offset,
        })
    }
}
