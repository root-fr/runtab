use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::model::{CostBasis, ToolResultSeen, ToolUseSeen, UsageEvent};

use super::claude_tools;
use super::{Adapter, ParseOutput};

const SOURCE: &str = "claude_code";

#[derive(Default)]
pub struct ClaudeCodeAdapter;

impl ClaudeCodeAdapter {
    pub fn new() -> ClaudeCodeAdapter {
        ClaudeCodeAdapter
    }
}

impl Adapter for ClaudeCodeAdapter {
    fn discover(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for root in discovery_roots() {
            collect_jsonl(&root, &mut files);
        }
        let mut seen = HashSet::new();
        files.retain(|p| {
            let key = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            seen.insert(key)
        });
        files
    }

    fn parse_from(&self, path: &Path, byte_offset: u64) -> io::Result<ParseOutput> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        // Resume only when the offset still lands on a line boundary (the byte
        // before it is a newline). A stale offset means the file was rewritten
        // or rotated, so re-read from the start — dedup makes that safe.
        let start = if byte_offset == 0 || byte_offset > len {
            0
        } else if is_line_boundary(&mut file, byte_offset)? {
            byte_offset
        } else {
            0
        };
        file.seek(SeekFrom::Start(start))?;

        let mut reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut tool_uses = Vec::new();
        let mut tool_results = Vec::new();
        let mut lines_skipped = 0u64;
        let mut consumed = start;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }
            // A line with no trailing newline is a partial append still being
            // written. Leave it (and its bytes) for the next scan so the offset
            // stays on a boundary and the line is parsed once it is complete.
            if buf.last() != Some(&b'\n') {
                break;
            }
            consumed += n as u64;
            match std::str::from_utf8(&buf) {
                Ok(line) => match parse_line(line, &mut tool_uses, &mut tool_results) {
                    LineOutcome::Event(e) => events.push(*e),
                    LineOutcome::Skip => lines_skipped += 1,
                    LineOutcome::Ignore => {}
                },
                // Non-UTF-8 bytes (torn multi-byte write, binary garbage): count
                // and skip the single line, never fail the whole file.
                Err(_) => lines_skipped += 1,
            }
        }
        Ok(ParseOutput {
            events,
            tool_uses,
            tool_results,
            lines_skipped,
            new_offset: consumed,
        })
    }
}

/// True when `offset` sits just after a newline, i.e. at a real line start.
fn is_line_boundary(file: &mut File, offset: u64) -> io::Result<bool> {
    file.seek(SeekFrom::Start(offset - 1))?;
    let mut b = [0u8; 1];
    file.read_exact(&mut b)?;
    Ok(b[0] == b'\n')
}

/// Combined discovery roots: `~/.claude/projects`,
/// `$XDG_CONFIG_HOME/claude/projects` (default `~/.config/claude/projects`),
/// and every comma-separated `$CLAUDE_CONFIG_DIR` entry + `/projects`.
fn discovery_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = crate::home_dir() {
        roots.push(home.join(".claude").join("projects"));
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| crate::home_dir().map(|h| h.join(".config")));
    if let Some(cfg) = xdg {
        roots.push(cfg.join("claude").join("projects"));
    }
    if let Some(dirs) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        for part in dirs.to_string_lossy().split(',') {
            let part = part.trim();
            if !part.is_empty() {
                roots.push(Path::new(part).join("projects"));
            }
        }
    }
    roots
}

fn collect_jsonl(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_jsonl(&path, out);
        } else if file_type.is_file()
            && path.extension().map(|e| e == "jsonl").unwrap_or(false)
        {
            out.push(path);
        }
    }
}

enum LineOutcome {
    // Boxed to keep the enum small — the other variants are zero-sized.
    Event(Box<UsageEvent>),
    /// Looked like a usage or tool record (widened substring pre-filter) but
    /// failed to parse or validate.
    Skip,
    /// Not a usage record (e.g. a user prompt); not an error, not counted.
    Ignore,
}

fn parse_line(
    line: &str,
    tool_uses: &mut Vec<ToolUseSeen>,
    tool_results: &mut Vec<ToolResultSeen>,
) -> LineOutcome {
    let admitted = line.contains("\"usage\"")
        || line.contains("\"tool_use\"")
        || line.contains("\"tool_result\"");
    if !admitted {
        return LineOutcome::Ignore;
    }
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return LineOutcome::Skip,
    };
    // Both extractors read the same parsed `Value` — a line can carry a
    // usage record and tool_use/tool_result blocks at once.
    let (uses, results) = claude_tools::scan_line(&v);
    tool_uses.extend(uses);
    tool_results.extend(results);
    match build_event(&v) {
        Some(Ok(event)) => LineOutcome::Event(Box::new(event)),
        Some(Err(())) => LineOutcome::Skip,
        None => LineOutcome::Ignore,
    }
}

/// `None` → not a usage record (ignored, uncounted). `Some(Err)` → a record that
/// carries a `usage` field but fails defensive validation (skipped, counted).
/// The transcript format is officially internal and changes between versions, so
/// anything malformed is dropped rather than trusted.
fn build_event(v: &Value) -> Option<Result<UsageEvent, ()>> {
    let msg = v.get("message").and_then(Value::as_object)?;
    if !msg.contains_key("usage") {
        return None;
    }
    Some(build_usage_event(v, msg))
}

fn build_usage_event(v: &Value, msg: &Map<String, Value>) -> Result<UsageEvent, ()> {
    // A present-but-non-object `usage` is a malformed usage record, not a
    // non-usage line, so it is skipped rather than silently ignored.
    let usage = msg.get("usage").and_then(Value::as_object).ok_or(())?;

    let model = str_field(msg.get("model"));
    let message_id = str_field(msg.get("id"));
    let session_id = str_field(v.get("sessionId"));
    let request_id = str_field(v.get("requestId"));
    let version = str_field(v.get("version"));
    let ts = str_field(v.get("timestamp"));

    if model.is_empty()
        || message_id.is_empty()
        || session_id.is_empty()
        || request_id.is_empty()
        || ts.is_empty()
        || !is_semver(&version)
    {
        return Err(());
    }

    // Claude Code writes a per-line `costUSD` in some API-billed configurations.
    // When present it is a real logged cost, preferred over any estimate and
    // used to auto-detect the source's `api` billing mode.
    let (cost_usd, cost_basis) = match v.get("costUSD").and_then(Value::as_f64) {
        Some(c) if c >= 0.0 => (Some(c), CostBasis::Logged),
        _ => (None, CostBasis::Estimated),
    };

    let cache = usage.get("cache_creation").and_then(Value::as_object);
    Ok(UsageEvent {
        source: SOURCE.to_string(),
        message_id,
        request_id,
        session_id,
        ts,
        model,
        input_tokens: token_field(usage.get("input_tokens"))?,
        output_tokens: token_field(usage.get("output_tokens"))?,
        cache_read_tokens: token_field(usage.get("cache_read_input_tokens"))?,
        cache_creation_tokens: token_field(usage.get("cache_creation_input_tokens"))?,
        cache_1h_tokens: cache
            .map(|c| int_field(c.get("ephemeral_1h_input_tokens")))
            .unwrap_or(0),
        cache_5m_tokens: cache
            .map(|c| int_field(c.get("ephemeral_5m_input_tokens")))
            .unwrap_or(0),
        reasoning_tokens: 0,
        project: project_from_cwd(&str_field(v.get("cwd"))),
        agent_version: version,
        cost_usd,
        cost_basis,
    })
}

pub(super) fn str_field(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").to_string()
}

fn int_field(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_i64).unwrap_or(0)
}

/// Missing or JSON null → 0. A present-but-non-numeric value (string, bool,
/// object) is a corrupt count, so it skips the whole event rather than silently
/// undercounting; integer and float encodings are both accepted.
fn token_field(v: Option<&Value>) -> Result<i64, ()> {
    match v {
        None | Some(Value::Null) => Ok(0),
        Some(value) => value
            .as_i64()
            .or_else(|| value.as_u64().map(|u| u as i64))
            .or_else(|| value.as_f64().map(|f| f as i64))
            .ok_or(()),
    }
}

/// Loose semver check: `MAJOR.MINOR.PATCH` core, each numeric, optional
/// pre-release / build metadata after `-` / `+`.
fn is_semver(v: &str) -> bool {
    let core = v.split('+').next().unwrap_or(v);
    let core = core.split('-').next().unwrap_or(core);
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Full project path from the transcript's `cwd`. The whole path is stored (only
/// a trailing slash is trimmed) so projects that share a directory name stay
/// distinct; shortening for readability is a display-time concern.
pub(super) fn project_from_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches('/');
    if trimmed.is_empty() {
        cwd.to_string()
    } else {
        trimmed.to_string()
    }
}
