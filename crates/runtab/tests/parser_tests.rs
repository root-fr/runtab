use std::path::{Path, PathBuf};

use runtab::adapters::{Adapter, ClaudeCodeAdapter};
use runtab::cmdnorm;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn parses_normal_events_and_ignores_non_usage_lines() {
    let out = ClaudeCodeAdapter::new()
        .parse_from(&fixture("claude_normal.jsonl"), 0)
        .unwrap();

    assert_eq!(out.events.len(), 3);
    assert_eq!(out.lines_skipped, 0);

    let e = &out.events[0];
    assert_eq!(e.source, "claude_code");
    assert_eq!(e.message_id, "m1");
    assert_eq!(e.request_id, "r1");
    assert_eq!(e.session_id, "s1");
    assert_eq!(e.model, "claude-sonnet-4-5-20250929");
    assert_eq!(e.project, "/home/u/projA");
    assert_eq!(e.agent_version, "1.2.3");
    assert_eq!(e.input_tokens, 100);
    assert_eq!(e.output_tokens, 50);
    assert_eq!(e.cache_read_tokens, 10);
}

#[test]
fn malformed_lines_are_counted_not_fatal() {
    let out = ClaudeCodeAdapter::new()
        .parse_from(&fixture("claude_malformed.jsonl"), 0)
        .unwrap();

    // 4 malformed usage records (bad json, empty session, bad version, empty id),
    // 1 valid event, 1 non-usage line ignored.
    assert_eq!(out.events.len(), 1);
    assert_eq!(out.lines_skipped, 4);
    assert_eq!(out.events[0].message_id, "mq4");
}

#[test]
fn extracts_ephemeral_cache_splits() {
    let out = ClaudeCodeAdapter::new()
        .parse_from(&fixture("claude_cache_heavy.jsonl"), 0)
        .unwrap();

    assert_eq!(out.events.len(), 1);
    let e = &out.events[0];
    assert_eq!(e.cache_creation_tokens, 1000);
    assert_eq!(e.cache_1h_tokens, 400);
    assert_eq!(e.cache_5m_tokens, 600);
    assert_eq!(e.cache_read_tokens, 5000);
}

#[test]
fn one_line_yields_both_usage_event_and_tool_use() {
    let out = ClaudeCodeAdapter::new()
        .parse_from(&fixture("claude_tool_events.jsonl"), 0)
        .unwrap();

    assert_eq!(out.lines_skipped, 0);

    // Line 1 carries both a usage record and a tool_use block.
    assert_eq!(out.events.len(), 1);
    assert_eq!(out.events[0].message_id, "mt1");

    // toolu_100 (line 1, Bash) + toolu_101 (line 4, Read, no usage on that line).
    assert_eq!(out.tool_uses.len(), 2);
    let bash_use = &out.tool_uses[0];
    assert_eq!(bash_use.tool_use_id, "toolu_100");
    assert_eq!(bash_use.session_id, "st1");
    assert_eq!(bash_use.project, "/home/u/projT");
    assert_eq!(bash_use.tool_name, "Bash");
    assert_eq!(
        bash_use.bash_head_hashes,
        Some(serde_json::to_string(&vec![cmdnorm::hash("git")]).unwrap())
    );
    assert_eq!(
        bash_use.bash_chain_hashes,
        Some(serde_json::to_string(&vec![cmdnorm::hash("git status")]).unwrap())
    );

    let read_use = &out.tool_uses[1];
    assert_eq!(read_use.tool_use_id, "toolu_101");
    assert_eq!(read_use.tool_name, "Read");
    assert_eq!(read_use.bash_head_hashes, None);
    assert_eq!(read_use.bash_chain_hashes, None);

    // The plain-string user prompt (line 2) contributes no tool events.
    assert_eq!(out.tool_results.len(), 1);
    let result = &out.tool_results[0];
    assert_eq!(result.tool_use_id, "toolu_100");
    assert!(!result.is_error);
    assert_eq!(result.est_result_tokens, "On branch main".len().div_ceil(4) as i64);
}
