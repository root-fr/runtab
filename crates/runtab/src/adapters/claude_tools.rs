//! Extracts `tool_use` / `tool_result` content blocks from an already-parsed
//! Claude Code transcript line, so `claude_code::parse_line` can pull both the
//! usage event and any tool events out of one `serde_json::from_str` call.

use serde_json::Value;

use crate::cmdnorm;
use crate::model::{ToolResultSeen, ToolUseSeen};

use super::claude_code::{project_from_cwd, str_field};

const SOURCE: &str = "claude_code";

/// Scans one parsed transcript line for `tool_use` / `tool_result` blocks.
/// The transcript format is internal/unstable, so anything malformed is
/// dropped rather than trusted: a line missing `sessionId`/`timestamp`
/// yields nothing, a `tool_use` missing `id`/`name` is skipped, a
/// `tool_result` missing `tool_use_id` is skipped — none of this is counted
/// anywhere (the caller's `lines_skipped` tracks malformed *usage* records
/// only).
pub(super) fn scan_line(v: &Value) -> (Vec<ToolUseSeen>, Vec<ToolResultSeen>) {
    let session_id = str_field(v.get("sessionId"));
    let ts = str_field(v.get("timestamp"));
    if session_id.is_empty() || ts.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let Some(blocks) = content_blocks(v) else {
        return (Vec::new(), Vec::new());
    };

    let project = project_from_cwd(&str_field(v.get("cwd")));
    let mut tool_uses = Vec::new();
    let mut tool_results = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => tool_uses.extend(build_tool_use(block, &session_id, &ts, &project)),
            Some("tool_result") => tool_results.extend(build_tool_result(block, &session_id, &ts)),
            _ => {}
        }
    }
    (tool_uses, tool_results)
}

/// `message.content` when it's an array of blocks. `None` for a plain-string
/// content (regular user prompts) or any other/missing shape — not an error.
fn content_blocks(v: &Value) -> Option<&Vec<Value>> {
    v.get("message")?.get("content")?.as_array()
}

fn build_tool_use(block: &Value, session_id: &str, ts: &str, project: &str) -> Option<ToolUseSeen> {
    let tool_use_id = str_field(block.get("id"));
    let tool_name = str_field(block.get("name"));
    if tool_use_id.is_empty() || tool_name.is_empty() {
        return None;
    }

    let input = block.get("input");
    let est_args_tokens = est_tokens(serialized_len(input));
    let (bash_head_hashes, bash_chain_hashes) = bash_hashes(&tool_name, input);

    Some(ToolUseSeen {
        source: SOURCE.to_string(),
        session_id: session_id.to_string(),
        tool_use_id,
        ts: ts.to_string(),
        project: project.to_string(),
        tool_name,
        est_args_tokens,
        bash_head_hashes,
        bash_chain_hashes,
    })
}

fn build_tool_result(block: &Value, session_id: &str, ts: &str) -> Option<ToolResultSeen> {
    let tool_use_id = str_field(block.get("tool_use_id"));
    if tool_use_id.is_empty() {
        return None;
    }
    let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    let est_result_tokens = est_tokens(result_content_len(block.get("content")));

    Some(ToolResultSeen {
        source: SOURCE.to_string(),
        session_id: session_id.to_string(),
        tool_use_id,
        ts: ts.to_string(),
        est_result_tokens,
        is_error,
    })
}

/// `Bash`'s `input.command`, hashed per chain segment via `cmdnorm`; every
/// other tool (or a `Bash` call with no `command` string, or an
/// empty/whitespace-only one) gets `None` for both.
fn bash_hashes(tool_name: &str, input: Option<&Value>) -> (Option<String>, Option<String>) {
    if tool_name != "Bash" {
        return (None, None);
    }
    let Some(command) = input.and_then(|i| i.get("command")).and_then(Value::as_str) else {
        return (None, None);
    };
    if command.trim().is_empty() {
        return (None, None);
    }
    (
        json_array(cmdnorm::chain_head_hashes(command)),
        json_array(cmdnorm::chain_hashes(command)),
    )
}

/// `ceil(bytes / 4)`, the shared token-estimate rule for both args and
/// result content.
fn est_tokens(bytes: usize) -> i64 {
    bytes.div_ceil(4) as i64
}

/// Byte length of `v` serialized as compact JSON; a missing field is treated
/// as "no args" (`{}`).
fn serialized_len(v: Option<&Value>) -> usize {
    match v {
        Some(v) => json_byte_len(v),
        None => 2, // "{}"
    }
}

/// A `Write` sink that only counts bytes, so measuring a serialized size
/// doesn't require allocating a duplicate string of a potentially huge value.
struct CountingWriter(usize);

impl std::io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Byte length `v` would occupy as compact JSON, without building the string.
fn json_byte_len(v: &Value) -> usize {
    let mut w = CountingWriter(0);
    serde_json::to_writer(&mut w, v).map(|()| w.0).unwrap_or(0)
}

fn json_array(items: Vec<String>) -> Option<String> {
    serde_json::to_string(&items).ok()
}

/// Result-content byte length: a plain string's own length; a
/// `[{"type":"text","text":"…"}]` array's summed `text` lengths; anything
/// else (an object, a mixed/empty array, a number, `null`) falls back to the
/// content's own serialized-JSON length.
fn result_content_len(content: Option<&Value>) -> usize {
    match content {
        None => 0,
        Some(Value::String(s)) => s.len(),
        Some(Value::Array(items)) if is_all_text_blocks(items) => items
            .iter()
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or("").len())
            .sum(),
        Some(other) => json_byte_len(other),
    }
}

fn is_all_text_blocks(items: &[Value]) -> bool {
    !items.is_empty()
        && items
            .iter()
            .all(|b| b.get("type").and_then(Value::as_str) == Some("text"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Value {
        serde_json::from_str(line).expect("test fixture must be valid JSON")
    }

    #[test]
    fn extracts_pairing_fields_from_tool_use_and_result() {
        let use_line = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-07-07T19:10:20.902Z","cwd":"/home/u/proj","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file":"a.rs"}}]}}"#,
        );
        let (uses, results) = scan_line(&use_line);
        assert_eq!(uses.len(), 1);
        assert!(results.is_empty());
        let u = &uses[0];
        assert_eq!(u.source, "claude_code");
        assert_eq!(u.session_id, "s1");
        assert_eq!(u.tool_use_id, "toolu_1");
        assert_eq!(u.ts, "2026-07-07T19:10:20.902Z");
        assert_eq!(u.project, "/home/u/proj");
        assert_eq!(u.tool_name, "Read");

        let result_line = parse(
            r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-07T19:10:21.500Z","cwd":"/home/u/proj","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}"#,
        );
        let (uses, results) = scan_line(&result_line);
        assert!(uses.is_empty());
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.source, "claude_code");
        assert_eq!(r.session_id, "s1");
        assert_eq!(r.tool_use_id, "toolu_1");
        assert_eq!(r.ts, "2026-07-07T19:10:21.500Z");
        assert!(!r.is_error);
    }

    #[test]
    fn non_tool_lines_are_ignored() {
        let plain_prompt = parse(
            r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-07T19:10:20.902Z","cwd":"/home/u/proj","message":{"role":"user","content":"please continue"}}"#,
        );
        let (uses, results) = scan_line(&plain_prompt);
        assert!(uses.is_empty());
        assert!(results.is_empty());
    }

    #[test]
    fn line_missing_session_id_or_timestamp_yields_nothing() {
        let missing_session = parse(
            r#"{"type":"assistant","timestamp":"2026-07-07T19:10:20.902Z","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#,
        );
        let (uses, results) = scan_line(&missing_session);
        assert!(uses.is_empty());
        assert!(results.is_empty());

        let missing_ts = parse(
            r#"{"type":"assistant","sessionId":"s1","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#,
        );
        let (uses, results) = scan_line(&missing_ts);
        assert!(uses.is_empty());
        assert!(results.is_empty());
    }

    #[test]
    fn tool_use_missing_id_or_name_is_dropped() {
        let missing_id = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
        );
        assert!(scan_line(&missing_id).0.is_empty());

        let missing_name = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","input":{"command":"ls"}}]}}"#,
        );
        assert!(scan_line(&missing_name).0.is_empty());
    }

    #[test]
    fn tool_result_missing_tool_use_id_is_dropped() {
        let missing_id = parse(
            r#"{"type":"user","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
        );
        assert!(scan_line(&missing_id).1.is_empty());
    }

    #[test]
    fn multiple_tool_use_blocks_in_one_line_yield_multiple_entries() {
        let line = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[
                {"type":"tool_use","id":"toolu_1","name":"Read","input":{}},
                {"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"ls"}}
            ]}}"#,
        );
        let (uses, _) = scan_line(&line);
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[0].tool_use_id, "toolu_1");
        assert_eq!(uses[1].tool_use_id, "toolu_2");
    }

    #[test]
    fn est_args_tokens_is_ceil_of_serialized_input_bytes_over_4() {
        let line = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file":"src/main.rs"}}]}}"#,
        );
        let (uses, _) = scan_line(&line);
        let serialized = serde_json::to_string(&serde_json::json!({"file": "src/main.rs"})).unwrap();
        let expected = serialized.len().div_ceil(4) as i64;
        assert_eq!(uses[0].est_args_tokens, expected);
    }

    #[test]
    fn est_args_tokens_treats_missing_input_as_empty_object() {
        let line = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Read"}]}}"#,
        );
        let (uses, _) = scan_line(&line);
        assert_eq!(uses[0].est_args_tokens, 1); // ceil(len("{}")=2 / 4) = 1
    }

    #[test]
    fn result_content_as_plain_string() {
        let line = parse(
            r#"{"type":"user","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"hello world"}]}}"#,
        );
        let (_, results) = scan_line(&line);
        assert_eq!(results[0].est_result_tokens, "hello world".len().div_ceil(4) as i64);
    }

    #[test]
    fn result_content_as_text_block_array_sums_text_lengths() {
        let line = parse(
            r#"{"type":"user","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"abcd"},{"type":"text","text":"efgh12"}]}]}}"#,
        );
        let (_, results) = scan_line(&line);
        // "abcd" (4) + "efgh12" (6) = 10 bytes -> ceil(10/4) = 3
        assert_eq!(results[0].est_result_tokens, 3);
    }

    #[test]
    fn result_content_other_shape_falls_back_to_serialized_json_length() {
        let line = parse(
            r#"{"type":"user","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":{"foo":"bar"}}]}}"#,
        );
        let (_, results) = scan_line(&line);
        let serialized = serde_json::to_string(&serde_json::json!({"foo": "bar"})).unwrap();
        let expected = serialized.len().div_ceil(4) as i64;
        assert_eq!(results[0].est_result_tokens, expected);
    }

    #[test]
    fn is_error_true_is_captured() {
        let line = parse(
            r#"{"type":"user","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"boom","is_error":true}]}}"#,
        );
        let (_, results) = scan_line(&line);
        assert!(results[0].is_error);
    }

    #[test]
    fn bash_input_gets_hash_arrays_from_cmdnorm() {
        let line = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cd /a && git status"}}]}}"#,
        );
        let (uses, _) = scan_line(&line);
        let u = &uses[0];
        let expected_heads =
            serde_json::to_string(&vec![cmdnorm::hash("cd"), cmdnorm::hash("git")]).unwrap();
        let expected_chains = serde_json::to_string(&vec![
            cmdnorm::hash("cd /a"),
            cmdnorm::hash("git status"),
        ])
        .unwrap();
        assert_eq!(u.bash_head_hashes, Some(expected_heads));
        assert_eq!(u.bash_chain_hashes, Some(expected_chains));
    }

    #[test]
    fn bash_env_only_segment_is_skipped_in_head_hashes_only() {
        let line = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"A=1;ls"}}]}}"#,
        );
        let (uses, _) = scan_line(&line);
        let u = &uses[0];
        let expected_heads = serde_json::to_string(&vec![cmdnorm::hash("ls")]).unwrap();
        assert_eq!(u.bash_head_hashes, Some(expected_heads));
        assert_eq!(cmdnorm::chain_hashes("A=1;ls").len(), 2);
    }

    #[test]
    fn non_bash_tool_gets_none_hashes() {
        let line = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file":"a.rs"}}]}}"#,
        );
        let (uses, _) = scan_line(&line);
        assert_eq!(uses[0].bash_head_hashes, None);
        assert_eq!(uses[0].bash_chain_hashes, None);
    }

    #[test]
    fn bash_without_command_field_is_treated_as_non_bash() {
        let line = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"description":"no command key"}}]}}"#,
        );
        let (uses, _) = scan_line(&line);
        assert_eq!(uses[0].bash_head_hashes, None);
        assert_eq!(uses[0].bash_chain_hashes, None);
    }

    #[test]
    fn bash_with_empty_or_blank_command_gets_none_hashes() {
        let empty = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":""}}]}}"#,
        );
        let (uses, _) = scan_line(&empty);
        assert_eq!(uses[0].bash_head_hashes, None);
        assert_eq!(uses[0].bash_chain_hashes, None);

        let blank = parse(
            r#"{"type":"assistant","sessionId":"s1","timestamp":"t","cwd":"/p","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"   "}}]}}"#,
        );
        let (uses, _) = scan_line(&blank);
        assert_eq!(uses[0].bash_head_hashes, None);
        assert_eq!(uses[0].bash_chain_hashes, None);
    }
}
