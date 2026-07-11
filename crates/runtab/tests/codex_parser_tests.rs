use std::path::{Path, PathBuf};

use runtab::adapters::{codex_discovery_roots_from, Adapter, CodexAdapter};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn parse(name: &str) -> runtab::adapters::ParseOutput {
    CodexAdapter.parse_from(&fixture(name), 0).unwrap()
}

#[test]
fn normal_session_emits_two_events_with_delta_semantics() {
    let out = parse("codex_normal.jsonl");

    // Two real token_count events (lines 4 and 5); the post-compaction
    // re-baseline (line 8) is dropped by the zero-component guard.
    assert_eq!(out.events.len(), 2);
    assert_eq!(out.lines_skipped, 0);

    let e0 = &out.events[0];
    assert_eq!(e0.source, "codex");
    assert_eq!(e0.session_id, "019104a2-1e3b-7c9a-8b7d-4e2f9a1c3d5e");
    assert_eq!(e0.message_id, "019104a2-1e3b-7c9a-8b7d-4e2f9a1c3d5e");
    // request_id is the cumulative total as a decimal string.
    assert_eq!(e0.request_id, "2160");
    assert_eq!(e0.model, "gpt-5.1-codex");
    assert_eq!(e0.ts, "2026-07-11T09:00:07.410Z");
    assert_eq!(e0.project, "/home/matthieu/Documents/Dev.local/tkm");
    assert_eq!(e0.agent_version, "0.145.0");
    // First call: input has no cache, so cache-exclusive input == input.
    assert_eq!(e0.input_tokens, 1820);
    assert_eq!(e0.output_tokens, 340);
    assert_eq!(e0.cache_read_tokens, 0);
    assert_eq!(e0.cache_creation_tokens, 0);
    assert_eq!(e0.reasoning_tokens, 120);

    // Event 2 is the last_token_usage delta, never the cumulative diff.
    let e1 = &out.events[1];
    assert_eq!(e1.request_id, "10960");
    // cache-exclusive input = 7830 - 4200 = 3630.
    assert_eq!(e1.input_tokens, 3630);
    assert_eq!(e1.output_tokens, 970);
    assert_eq!(e1.cache_read_tokens, 4200);
    assert_eq!(e1.reasoning_tokens, 290);
    assert_eq!(e1.cache_creation_tokens, 0);
    assert_eq!(e1.cache_1h_tokens, 0);
    assert_eq!(e1.cache_5m_tokens, 0);
}

#[test]
fn compaction_rebaseline_emits_nothing() {
    // The post-compaction token_count (line 8) has all-zero last_token_usage
    // components and an estimated total_tokens: the zero-component guard drops
    // it, so only the two genuine events remain.
    let out = parse("codex_normal.jsonl");
    assert_eq!(out.events.len(), 2);
    assert!(out.events.iter().all(|e| e.total_tokens() > 0));
}

#[test]
fn model_switch_attributes_per_turn_context() {
    let out = parse("codex_model_switch.jsonl");
    assert_eq!(out.events.len(), 3);
    assert_eq!(out.lines_skipped, 0);

    // token_count before the first turn_context keeps its tokens under "unknown".
    assert_eq!(out.events[0].model, "unknown");
    assert_eq!(out.events[0].input_tokens, 500);
    assert_eq!(out.events[0].reasoning_tokens, 40);

    // After the first turn_context.
    assert_eq!(out.events[1].model, "gpt-5.1-codex");
    // cache-exclusive input = 2000 - 500 = 1500.
    assert_eq!(out.events[1].input_tokens, 1500);
    assert_eq!(out.events[1].cache_read_tokens, 500);

    // After the /model switch turn_context.
    assert_eq!(out.events[2].model, "gpt-5.1");
    assert_eq!(out.events[2].input_tokens, 2500 - 700);
    assert_eq!(out.events[2].cache_read_tokens, 700);
}

#[test]
fn info_null_ignored_uncounted_and_no_events() {
    let out = parse("codex_no_usage.jsonl");
    assert_eq!(out.events.len(), 0);
    assert_eq!(out.lines_skipped, 0);
}

#[test]
fn malformed_lines_counted_and_pre_session_usage_skipped() {
    let out = parse("codex_malformed.jsonl");
    // Exactly one valid event survives.
    assert_eq!(out.events.len(), 1);
    assert_eq!(out.events[0].input_tokens, 2000 - 300);
    assert_eq!(out.events[0].cache_read_tokens, 300);
    // (1) token_count before session_meta (unattributable),
    // (2) non-numeric token field, (3) broken JSON on an admitted line.
    assert_eq!(out.lines_skipped, 3);
}

#[test]
fn fork_replay_drops_replay_second_keeps_genuine_event() {
    let out = parse("codex_fork_replay.jsonl");
    // The first run of >=2 consecutive same-second token_counts is the replay
    // second; every event in that second is dropped. Only the genuine later
    // event (a different second) survives.
    assert_eq!(out.events.len(), 1);
    assert_eq!(out.events[0].ts, "2026-07-11T12:30:52.500Z");
    // Its own last_token_usage delta, cache-exclusive.
    assert_eq!(out.events[0].input_tokens, 3500 - 1000);
    assert_eq!(out.events[0].cache_read_tokens, 1000);
    assert_eq!(out.events[0].output_tokens, 700);
    assert_eq!(out.events[0].reasoning_tokens, 200);
    assert_eq!(out.events[0].request_id, "242200");
}

#[test]
fn unflagged_same_second_burst_all_kept() {
    // codex_normal is unflagged; its events at distinct seconds all emit. The
    // heuristic only fires on flagged (fork/subagent) files, so an unflagged
    // file never drops same-second events.
    let out = parse("codex_normal.jsonl");
    assert_eq!(out.events.len(), 2);
}

#[test]
fn later_meta_clearing_flag_does_not_reprieve_earlier_replay_burst() {
    // Two session_meta segments (spec §4.2/§4.4): segment 1 is a fork (flagged)
    // whose two token_counts share a second — a replayed parent burst that MUST
    // be dropped. Segment 2 is a plain (unflagged) session whose two distinct-
    // second events are genuine and MUST emit. The heuristic gates on each
    // segment's own flag at collection time, so clearing the flag on the second
    // session_meta cannot resurrect the earlier flagged burst.
    let out = parse("codex_two_meta_flag_off.jsonl");
    assert_eq!(out.events.len(), 2, "only segment 2's genuine events survive");
    assert_eq!(out.events[0].request_id, "10900");
    assert_eq!(out.events[0].input_tokens, 10000 - 2000);
    assert_eq!(out.events[1].request_id, "16400");
    assert_eq!(out.events[1].input_tokens, 5000 - 1000);
    // The flagged same-second parent burst (108000/194000) is gone.
    assert!(out
        .events
        .iter()
        .all(|e| e.request_id != "108000" && e.request_id != "194000"));
}

#[test]
fn later_meta_setting_flag_does_not_drop_earlier_genuine_burst() {
    // Segment 1 is a plain (unflagged) session with a genuine same-second pair
    // that MUST all emit. Segment 2 is a subagent (flagged) with a replayed
    // same-second burst (two events) plus one genuine later-second event. The
    // per-segment flag keeps segment 1 untouched and drops only segment 2's
    // replay second.
    let out = parse("codex_two_meta_flag_on.jsonl");
    assert_eq!(out.events.len(), 3);
    // Segment 1: both genuine same-second events kept.
    assert_eq!(out.events[0].request_id, "43000");
    assert_eq!(out.events[1].request_id, "96000");
    // Segment 2: only the genuine later-second event survives.
    assert_eq!(out.events[2].request_id, "242200");
    assert_eq!(out.events[2].ts, "2026-07-11T15:10:52.500Z");
    // Segment 2's replayed burst (108000/194000) is dropped.
    assert!(out
        .events
        .iter()
        .all(|e| e.request_id != "108000" && e.request_id != "194000"));
}

#[test]
fn multibyte_timestamp_skips_line_without_panicking() {
    // A rollout line whose timestamp carries a multibyte char straddling byte 19
    // must be fail-soft (lines_skipped), never a panic that aborts the scan.
    let out = parse("codex_multibyte_ts.jsonl");
    // Only the well-formed later line survives; the multibyte-ts line is counted.
    assert_eq!(out.events.len(), 1);
    assert_eq!(out.lines_skipped, 1);
    assert_eq!(out.events[0].request_id, "2400");
    assert_eq!(out.events[0].input_tokens, 2000 - 300);
}

#[test]
fn parse_from_returns_file_length_as_new_offset() {
    let path = fixture("codex_normal.jsonl");
    let len = std::fs::metadata(&path).unwrap().len();
    let out = CodexAdapter.parse_from(&path, 0).unwrap();
    assert_eq!(out.new_offset, len);
}

#[test]
fn openai_prefixes_resolve_longest_match() {
    use runtab::model::{CostBasis, UsageEvent};
    use runtab::pricing::Pricing;
    use std::collections::BTreeSet;

    let pricing = Pricing::load().unwrap();

    fn priced(pricing: &Pricing, model: &str) -> f64 {
        let mut e = UsageEvent {
            source: "codex".to_string(),
            message_id: "m".to_string(),
            request_id: "r".to_string(),
            session_id: "s".to_string(),
            ts: "2026-07-11T09:00:00.000Z".to_string(),
            model: model.to_string(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cache_1h_tokens: 0,
            cache_5m_tokens: 0,
            reasoning_tokens: 0,
            project: "p".to_string(),
            agent_version: String::new(),
            cost_usd: None,
            cost_basis: CostBasis::Estimated,
        };
        let mut unknown = BTreeSet::new();
        pricing.apply(&mut e, &mut unknown);
        assert!(unknown.is_empty(), "{model} should be priced");
        e.cost_usd.unwrap()
    }

    // Each more-specific prefix must beat the broader family entry.
    let mini = priced(&pricing, "gpt-5.1-codex-mini");
    let codex = priced(&pricing, "gpt-5.1-codex");
    let five_one = priced(&pricing, "gpt-5.1-2026-01-01");
    let five = priced(&pricing, "gpt-5-2025-08-01");
    let nano = priced(&pricing, "gpt-5-nano");

    assert!((mini - 0.25).abs() < 1e-9, "codex-mini input was {mini}");
    assert!((codex - 1.25).abs() < 1e-9, "codex input was {codex}");
    assert!((five_one - 1.25).abs() < 1e-9, "gpt-5.1 input was {five_one}");
    assert!((five - 1.25).abs() < 1e-9, "gpt-5 input was {five}");
    assert!((nano - 0.05).abs() < 1e-9, "gpt-5-nano input was {nano}");
    // codex-mini is strictly cheaper than codex; nano cheaper than the family.
    assert!(mini < codex);
    assert!(nano < five);
}

#[test]
fn codex_cached_input_prices_at_cache_read_rate() {
    use runtab::model::{CostBasis, UsageEvent};
    use runtab::pricing::Pricing;
    use std::collections::BTreeSet;

    let pricing = Pricing::load().unwrap();
    let mut e = UsageEvent {
        source: "codex".to_string(),
        message_id: "m".to_string(),
        request_id: "r".to_string(),
        session_id: "s".to_string(),
        ts: "2026-07-11T09:00:00.000Z".to_string(),
        model: "gpt-5.1-codex".to_string(),
        input_tokens: 0,
        output_tokens: 0,
        // cached_input_tokens land in cache_read_tokens per the field mapping.
        cache_read_tokens: 1_000_000,
        cache_creation_tokens: 1_000_000,
        cache_1h_tokens: 0,
        cache_5m_tokens: 0,
        reasoning_tokens: 0,
        project: "p".to_string(),
        agent_version: String::new(),
        cost_usd: None,
        cost_basis: CostBasis::Estimated,
    };
    let mut unknown = BTreeSet::new();
    pricing.apply(&mut e, &mut unknown);
    // cache read billed at 0.125/M; OpenAI bills no cache writes → creation free.
    let cost = e.cost_usd.unwrap();
    assert!((cost - 0.125).abs() < 1e-9, "cost was {cost}");
}

#[test]
fn discovery_roots_prefers_codex_home_over_default() {
    let codex_home = PathBuf::from("/custom/codex");
    let home = PathBuf::from("/home/u");
    let roots = codex_discovery_roots_from(Some(codex_home.clone()), Some(home.clone()));
    // sessions/ and archived_sessions/ under the override root, not ~/.codex.
    assert!(roots.contains(&codex_home.join("sessions")));
    assert!(roots.contains(&codex_home.join("archived_sessions")));
    assert!(roots.iter().all(|r| r.starts_with(&codex_home)));
}

#[test]
fn discovery_roots_falls_back_to_home_dot_codex() {
    let home = PathBuf::from("/home/u");
    let roots = codex_discovery_roots_from(None, Some(home.clone()));
    let dot = home.join(".codex");
    assert!(roots.contains(&dot.join("sessions")));
    assert!(roots.contains(&dot.join("archived_sessions")));
}

#[test]
fn discovery_roots_empty_without_home_or_override() {
    let roots = codex_discovery_roots_from(None, None);
    assert!(roots.is_empty());
}
