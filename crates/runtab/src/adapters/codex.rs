use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::{CostBasis, UsageEvent};

use super::claude_code::{project_from_cwd, str_field};
use super::{Adapter, ParseOutput};

const SOURCE: &str = "codex";

/// Codex CLI adapter. Rollouts are append-only JSONL (optionally `.jsonl.zst`);
/// parsing is file-context-dependent (session_meta at line 1, model from the
/// last `turn_context`, replay detection needs the file prefix), so the
/// `byte_offset` is a parsed-through-length sentinel rather than a resume point:
/// a full re-parse from 0 runs whenever the file has grown, and each event's
/// deterministic identity dedups the re-read rows to zero inserts.
#[derive(Default)]
pub struct CodexAdapter;

impl Adapter for CodexAdapter {
    fn discover(&self) -> Vec<PathBuf> {
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty() && p.is_dir());
        let mut files = Vec::new();
        for root in codex_discovery_roots_from(codex_home, crate::home_dir()) {
            collect_rollouts(&root, &mut files);
        }
        files
    }

    fn parse_from(&self, path: &Path, byte_offset: u64) -> io::Result<ParseOutput> {
        let is_zst = path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("zst"))
            .unwrap_or(false);

        // Compressed rollouts are immutable once written: any non-zero offset
        // means we already read the whole file.
        if is_zst {
            if byte_offset > 0 {
                return Ok(empty(byte_offset));
            }
            let compressed = fs::read(path)?;
            let len = compressed.len() as u64;
            let decoder = zstd::stream::read::Decoder::new(&compressed[..])?;
            return Ok(parse_reader(BufReader::new(decoder), len));
        }

        let len = fs::metadata(path)?.len();
        // Unchanged-file fast path: the stored offset is the length we observed
        // last time, so an offset at or past the current length means nothing
        // was appended. A dead file is then skipped forever instead of being
        // re-parsed every tick.
        if byte_offset > 0 && byte_offset >= len {
            return Ok(empty(byte_offset));
        }
        let file = File::open(path)?;
        Ok(parse_reader(BufReader::new(file), len))
    }
}

fn empty(offset: u64) -> ParseOutput {
    ParseOutput {
        events: Vec::new(),
        tool_uses: Vec::new(),
        tool_results: Vec::new(),
        lines_skipped: 0,
        new_offset: offset,
    }
}

/// Discovery roots under the Codex home: `sessions/` (a YYYY/MM/DD tree) and
/// `archived_sessions/` (a flat rename target with byte-identical content).
/// `$CODEX_HOME` wins when set; otherwise `~/.codex`. Pure so tests never touch
/// process env or the developer's real `~/.codex`.
pub fn codex_discovery_roots_from(
    codex_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let root = codex_home.or_else(|| home.map(|h| h.join(".codex")));
    let Some(root) = root else {
        return Vec::new();
    };
    vec![root.join("sessions"), root.join("archived_sessions")]
}

fn collect_rollouts(root: &Path, out: &mut Vec<PathBuf>) {
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
            collect_rollouts(&path, out);
        } else if file_type.is_file() && is_rollout(&path) {
            out.push(path);
        }
    }
}

fn is_rollout(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    // `.jsonl` or `.jsonl.zst`; `history.jsonl` and the SQLite state DB carry no
    // token data / are a derived cache, so they are excluded.
    name != "history.jsonl" && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
}

/// A parsed token_count event held with its second-truncated timestamp so the
/// fork/subagent replay heuristic can run over the buffered per-file events.
struct Candidate {
    event: UsageEvent,
    second: String,
}

/// The token_counts collected under one `session_meta` line, together with the
/// `replay_suspect` flag as evaluated for THAT `session_meta`. The replay
/// heuristic (spec §4.4) runs per segment against the flag captured at
/// collection time — a later `session_meta` (a subagent boundary, §4.2) that
/// flips the flag must not retroactively change how earlier events are handled.
struct Segment {
    replay_suspect: bool,
    candidates: Vec<Candidate>,
}

/// Per-file parse state. Codex parsing is a single pass over the whole file; a
/// later `session_meta` line (a subagent boundary) re-runs extraction and
/// opens a new segment with its own `replay_suspect` flag.
struct ParseState {
    session_id: Option<String>,
    cwd: String,
    agent_version: String,
    model: String,
    segments: Vec<Segment>,
    lines_skipped: u64,
}

impl ParseState {
    fn new() -> ParseState {
        ParseState {
            session_id: None,
            cwd: String::new(),
            agent_version: String::new(),
            model: "unknown".to_string(),
            segments: Vec::new(),
            lines_skipped: 0,
        }
    }

    /// Buffer a token_count under the current segment. A token_count seen before
    /// any `session_meta` cannot reach here (it is skipped for want of a
    /// `session_id`), so an open segment always exists at push time.
    fn push_candidate(&mut self, candidate: Candidate) {
        if let Some(segment) = self.segments.last_mut() {
            segment.candidates.push(candidate);
        }
    }
}

fn parse_reader<R: BufRead>(mut reader: R, len: u64) -> ParseOutput {
    let mut state = ParseState::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            // A read error mid-file (torn decode, I/O fault): stop here with
            // what parsed cleanly, same fail-soft posture as a torn tail.
            Err(_) => break,
        }
        // A final line with no trailing newline is a partial append still being
        // written; leave it for the growth-triggered reparse (codex always
        // re-reads from 0 when the file grows).
        if buf.last() != Some(&b'\n') {
            break;
        }
        match std::str::from_utf8(&buf) {
            Ok(line) => handle_line(line.trim_end_matches(['\n', '\r']), &mut state),
            Err(_) => state.lines_skipped += 1,
        }
    }

    let events = finalize(state.segments);
    ParseOutput {
        events,
        tool_uses: Vec::new(),
        tool_results: Vec::new(),
        lines_skipped: state.lines_skipped,
        new_offset: len,
    }
}

fn handle_line(line: &str, state: &mut ParseState) {
    // Substring pre-filter before the JSON parse: only these record types carry
    // the fields we read. Everything else is ignored, uncounted.
    let admitted = line.contains("\"session_meta\"")
        || line.contains("\"turn_context\"")
        || line.contains("\"compacted\"")
        || line.contains("\"token_count\"");
    if !admitted {
        return;
    }
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        // Malformed JSON on an admitted line is a usage-shaped line we cannot
        // trust: counted, not fatal.
        Err(_) => {
            if is_token_count(line) {
                state.lines_skipped += 1;
            }
            return;
        }
    };
    let record_type = v.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = v.get("payload");
    let payload_type = payload.and_then(|p| p.get("type")).and_then(Value::as_str);
    match record_type {
        "session_meta" => apply_session_meta(payload, state),
        "turn_context" => apply_turn_context(payload, state),
        "event_msg" if payload_type == Some("token_count") => {
            apply_token_count(&v, payload, state)
        }
        _ => {}
    }
}

/// A best-effort check used only to decide whether a JSON-invalid admitted line
/// should be counted as skipped: the substring pre-filter already required one
/// of the four record markers, and only token_count lines carry usage.
fn is_token_count(line: &str) -> bool {
    line.contains("\"token_count\"")
}

fn apply_session_meta(payload: Option<&Value>, state: &mut ParseState) {
    let Some(p) = payload else {
        return;
    };
    // `id` is the canonical thread id; legacy files may only carry `session_id`.
    let id = str_field(p.get("id"));
    let session_id = str_field(p.get("session_id"));
    let chosen = if !id.is_empty() {
        id
    } else if !session_id.is_empty() {
        session_id
    } else {
        String::new()
    };
    if !chosen.is_empty() {
        state.session_id = Some(chosen);
    }
    let cwd = str_field(p.get("cwd"));
    if !cwd.is_empty() {
        state.cwd = cwd;
    }
    state.agent_version = str_field(p.get("cli_version"));
    // A new session_meta opens a fresh segment; its token_counts are held and
    // judged against this session_meta's own replay flag (spec §4.4).
    state.segments.push(Segment {
        replay_suspect: is_replay_suspect(p),
        candidates: Vec::new(),
    });
}

/// Flagged when the session is a fork (`forked_from_id` non-null) or a subagent
/// (`source` is the `{"subagent": ...}` object form). ccusage #950/#1218/#1369:
/// such files can carry a verbatim replay of the parent's token history.
fn is_replay_suspect(p: &Value) -> bool {
    if p.get("forked_from_id")
        .map(|v| !v.is_null())
        .unwrap_or(false)
    {
        return true;
    }
    matches!(p.get("source"), Some(Value::Object(o)) if o.contains_key("subagent"))
}

fn apply_turn_context(payload: Option<&Value>, state: &mut ParseState) {
    let Some(p) = payload else {
        return;
    };
    let model = str_field(p.get("model"));
    if !model.is_empty() {
        state.model = model;
    }
    // `turn_context` repeats `cwd` (per-turn; can change if the turn's working
    // directory differs).
    let cwd = str_field(p.get("cwd"));
    if !cwd.is_empty() {
        state.cwd = cwd;
    }
}

fn apply_token_count(line: &Value, payload: Option<&Value>, state: &mut ParseState) {
    let Some(payload) = payload else {
        state.lines_skipped += 1;
        return;
    };
    let info = payload.get("info");
    // `info: null` is "no usage data yet" — ignored, uncounted (not zero, not
    // skipped). A missing `info` key is treated the same.
    match info {
        None | Some(Value::Null) => return,
        Some(_) => {}
    }
    let info = info.unwrap();

    let last = info.get("last_token_usage");
    let total = info
        .get("total_token_usage")
        .and_then(|t| t.get("total_tokens"));

    // Usage-shaped but unattributable / corrupt: counted, not fatal.
    let (Some(last), Some(total)) = (last, total) else {
        state.lines_skipped += 1;
        return;
    };
    let Some(cumulative) = as_token(Some(total)) else {
        state.lines_skipped += 1;
        return;
    };

    let input = match as_token(last.get("input_tokens")) {
        Some(v) => v,
        None => {
            state.lines_skipped += 1;
            return;
        }
    };
    let cached = match as_token(last.get("cached_input_tokens")) {
        Some(v) => v,
        None => {
            state.lines_skipped += 1;
            return;
        }
    };
    let output = match as_token(last.get("output_tokens")) {
        Some(v) => v,
        None => {
            state.lines_skipped += 1;
            return;
        }
    };
    let reasoning = match as_token(last.get("reasoning_output_tokens")) {
        Some(v) => v,
        None => {
            state.lines_skipped += 1;
            return;
        }
    };

    // Zero-component guard: a post-compaction/resume re-baseline writes all-zero
    // components with an estimated total — emit nothing. (The deterministic
    // identity also collapses re-baselines, this is belt and suspenders.)
    if input + cached + output + reasoning == 0 {
        return;
    }

    let ts = str_field(line.get("timestamp"));
    // A non-string / too-short timestamp is corrupt: skipped, counted. `get`
    // (not `ts[..19]`) so a multibyte char straddling byte 19 skips the line
    // instead of panicking the whole scan.
    let Some(second) = ts.get(..19).map(str::to_string) else {
        state.lines_skipped += 1;
        return;
    };
    let Some(session_id) = state.session_id.clone() else {
        // A token_count before any session_meta is usage-shaped but has no
        // session to attribute to: skipped, counted.
        state.lines_skipped += 1;
        return;
    };

    let event = UsageEvent {
        source: SOURCE.to_string(),
        message_id: session_id.clone(),
        request_id: cumulative.to_string(),
        session_id,
        ts,
        model: state.model.clone(),
        // Cache-exclusive input (claude shape): codex `input_tokens` includes
        // `cached_input_tokens` (OpenAI semantics), so subtract it, floor 0.
        input_tokens: (input - cached).max(0),
        output_tokens: output,
        cache_read_tokens: cached,
        // OpenAI reports/bills no cache writes.
        cache_creation_tokens: 0,
        cache_1h_tokens: 0,
        cache_5m_tokens: 0,
        // reasoning is a subset of output natively; the wire clamp is a no-op.
        reasoning_tokens: reasoning,
        project: project_from_cwd(&state.cwd),
        agent_version: state.agent_version.clone(),
        // Rollouts carry no cost; ChatGPT-plan and API-key Codex are
        // indistinguishable here, so both land Estimated.
        cost_usd: None,
        cost_basis: CostBasis::Estimated,
    };
    state.push_candidate(Candidate { event, second });
}

/// Numeric token field: JSON null / missing → 0; a present-but-non-numeric value
/// (string, bool, object) is corrupt → `None` so the caller skips the event.
/// Integer and float encodings are both accepted.
fn as_token(v: Option<&Value>) -> Option<i64> {
    match v {
        None | Some(Value::Null) => Some(0),
        Some(value) => value
            .as_i64()
            .or_else(|| value.as_u64().map(|u| u as i64))
            .or_else(|| value.as_f64().map(|f| f as i64)),
    }
}

/// Apply the fork/subagent replay heuristic (§4.4) to each `session_meta`
/// segment against the flag captured when that segment's events were collected,
/// then concatenate the survivors in file order. Segmenting is what keeps a
/// later session_meta's flag from re-judging earlier events (spec §4.2/§4.4).
fn finalize(segments: Vec<Segment>) -> Vec<UsageEvent> {
    let mut events = Vec::new();
    for segment in segments {
        finalize_segment(segment.candidates, segment.replay_suspect, &mut events);
    }
    events
}

/// One segment's survivors, appended to `out`. Only runs the heuristic when the
/// segment was flagged; unflagged segments emit every candidate.
fn finalize_segment(candidates: Vec<Candidate>, replay_suspect: bool, out: &mut Vec<UsageEvent>) {
    if !replay_suspect || candidates.is_empty() {
        out.extend(candidates.into_iter().map(|c| c.event));
        return;
    }

    // Replay-second detection: the FIRST run of >=2 consecutive events sharing
    // the same second-truncated timestamp marks that second as the replay
    // second. We drop every event in that second.
    let replay_second = candidates
        .windows(2)
        .find(|w| w[0].second == w[1].second)
        .map(|w| w[0].second.clone());

    if let Some(replay) = replay_second {
        out.extend(
            candidates
                .into_iter()
                .filter(|c| c.second != replay)
                .map(|c| c.event),
        );
        return;
    }

    // No replay second yet: hold back the trailing run (length >= 1) of
    // same-second events. When the file grows, the growth-triggered reparse
    // re-evaluates the run with more context — it either becomes the detected
    // replay second (dropped) or is followed by a different second (emitted).
    let last_second = candidates[candidates.len() - 1].second.clone();
    let keep = candidates
        .iter()
        .rposition(|c| c.second != last_second)
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut trailing = candidates;
    trailing.truncate(keep);
    out.extend(trailing.into_iter().map(|c| c.event));
}
