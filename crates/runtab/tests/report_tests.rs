mod common;

use common::{ev, insert};
use runtab::cmdnorm;
use runtab::ledger::Ledger;
use runtab::model::CostBasis::Estimated;
use runtab::model::{RtkCommandRow, ToolResultSeen, ToolUseSeen};
use runtab::rtkimport::attribute;

fn bash_use(session: &str, tool_use_id: &str, project: &str, cmd: &str, ts: &str) -> ToolUseSeen {
    ToolUseSeen {
        source: "claude_code".to_string(),
        session_id: session.to_string(),
        tool_use_id: tool_use_id.to_string(),
        ts: ts.to_string(),
        project: project.to_string(),
        tool_name: "Bash".to_string(),
        est_args_tokens: 5,
        bash_head_hashes: Some(serde_json::to_string(&cmdnorm::chain_head_hashes(cmd)).unwrap()),
        bash_chain_hashes: Some(serde_json::to_string(&cmdnorm::chain_hashes(cmd)).unwrap()),
    }
}

fn read_use(session: &str, tool_use_id: &str, project: &str, ts: &str) -> ToolUseSeen {
    ToolUseSeen {
        source: "claude_code".to_string(),
        session_id: session.to_string(),
        tool_use_id: tool_use_id.to_string(),
        ts: ts.to_string(),
        project: project.to_string(),
        tool_name: "Read".to_string(),
        est_args_tokens: 4,
        bash_head_hashes: None,
        bash_chain_hashes: None,
    }
}

fn result(session: &str, tool_use_id: &str, ts: &str, est_result_tokens: i64) -> ToolResultSeen {
    ToolResultSeen {
        source: "claude_code".to_string(),
        session_id: session.to_string(),
        tool_use_id: tool_use_id.to_string(),
        ts: ts.to_string(),
        est_result_tokens,
        is_error: false,
    }
}

#[test]
fn sessions_projects_and_tools_report_rtk_savings_and_tool_totals() {
    let l = Ledger::open_in_memory().unwrap();

    // Two sessions in projA, one session in projB.
    insert(&l, &ev("m1", "s1", "/home/u/projA", "model-x", "2026-07-01T10:00:00Z", 100, Estimated, None));
    insert(&l, &ev("m2", "s2", "/home/u/projA", "model-x", "2026-07-01T11:00:00Z", 100, Estimated, None));
    insert(&l, &ev("m3", "s3", "/home/u/projB", "model-x", "2026-07-01T12:00:00Z", 100, Estimated, None));

    // s1 ran a Bash call (later matched to an rtk row) and a Read call.
    l.insert_pending_tool_use(&bash_use("s1", "toolu_bash", "/home/u/projA", "git status", "2026-07-01T10:00:05Z"))
        .unwrap();
    assert!(l.resolve_tool_result(&result("s1", "toolu_bash", "2026-07-01T10:00:06Z", 20)).unwrap());

    l.insert_pending_tool_use(&read_use("s1", "toolu_read", "/home/u/projA", "2026-07-01T10:01:00Z")).unwrap();
    assert!(l.resolve_tool_result(&result("s1", "toolu_read", "2026-07-01T10:01:01Z", 40)).unwrap());

    // One rtk row that attributes to s1 by exact text match, one that stays
    // unattributed in projB (no Bash tool_events anywhere near it).
    let attributable = RtkCommandRow {
        rtk_row_id: 1,
        ts: "2026-07-01T10:00:06.500000000+00:00".to_string(),
        project_path: "/home/u/projA".to_string(),
        head_hash: cmdnorm::hash(&cmdnorm::head("git status")),
        cmd_hash: cmdnorm::hash("git status"),
        raw_tokens: 500,
        filtered_tokens: 100,
        saved_tokens: 400,
        exec_time_ms: 10,
    };
    assert!(l.insert_rtk_event(&attributable).unwrap());

    let unmatched = RtkCommandRow {
        rtk_row_id: 2,
        ts: "2026-07-01T12:05:00.000000000+00:00".to_string(),
        project_path: "/home/u/projB".to_string(),
        head_hash: cmdnorm::hash(&cmdnorm::head("some-other-cmd")),
        cmd_hash: cmdnorm::hash("some-other-cmd"),
        raw_tokens: 50,
        filtered_tokens: 10,
        saved_tokens: 40,
        exec_time_ms: 5,
    };
    assert!(l.insert_rtk_event(&unmatched).unwrap());

    let attribution = attribute(&l).unwrap();
    assert_eq!((attribution.text, attribution.window, attribution.none), (1, 0, 1));

    // sessions(): only s1 carries the attributed savings.
    let sessions = l.sessions().unwrap();
    assert_eq!(sessions.iter().find(|r| r.key == "s1").unwrap().saved_tokens, Some(400));
    assert_eq!(sessions.iter().find(|r| r.key == "s2").unwrap().saved_tokens, None);
    assert_eq!(sessions.iter().find(|r| r.key == "s3").unwrap().saved_tokens, None);

    // projects(): projA gets the attributed row; projB gets the unattributed
    // one too, since project grouping is independent of attribution.
    let projects = l.projects().unwrap();
    assert_eq!(projects.iter().find(|r| r.key == "/home/u/projA").unwrap().saved_tokens, Some(400));
    assert_eq!(projects.iter().find(|r| r.key == "/home/u/projB").unwrap().saved_tokens, Some(40));

    // daily()/models() never carry a saved_tokens value.
    assert!(l.daily().unwrap().iter().all(|r| r.saved_tokens.is_none()));
    assert!(l.models().unwrap().iter().all(|r| r.saved_tokens.is_none()));

    // tools(): exact calls/sums/share per tool_name.
    let tools = l.tool_aggregates(None, None).unwrap();
    let bash = tools.iter().find(|r| r.tool_name == "Bash").unwrap();
    assert_eq!(bash.calls, 1);
    assert_eq!(bash.est_args_tokens, 5);
    assert_eq!(bash.est_result_tokens, 20);
    assert_eq!(bash.est_total_tokens, 25);

    let read = tools.iter().find(|r| r.tool_name == "Read").unwrap();
    assert_eq!(read.calls, 1);
    assert_eq!(read.est_args_tokens, 4);
    assert_eq!(read.est_result_tokens, 40);
    assert_eq!(read.est_total_tokens, 44);

    let grand_total = 25.0 + 44.0;
    assert!((bash.share_pct - 100.0 * 25.0 / grand_total).abs() < 1e-9);
    assert!((read.share_pct - 100.0 * 44.0 / grand_total).abs() < 1e-9);

    let rtk_totals = l.rtk_totals(None, None).unwrap().unwrap();
    assert_eq!(rtk_totals.commands, 2);
    assert_eq!(rtk_totals.saved_tokens, 440);
}

#[test]
fn tool_aggregates_and_rtk_totals_are_none_and_empty_on_a_fresh_ledger() {
    let l = Ledger::open_in_memory().unwrap();
    assert!(l.tool_aggregates(None, None).unwrap().is_empty());
    assert!(l.rtk_totals(None, None).unwrap().is_none());
}

#[test]
fn tool_aggregates_and_rtk_totals_filter_by_days_and_session() {
    let l = Ledger::open_in_memory().unwrap();

    // s1 ran `git status` 400+ days ago; s2 ran `ls` an hour ago. Both Bash
    // calls so the real matcher (not a test-only shortcut) attributes each
    // rtk row by exact text match.
    let old_ts = "2000-01-01T10:00:00Z";
    let recent_ts = runtab::timeutil::epoch_to_rfc3339(runtab::timeutil::now_epoch() - 3600);

    l.insert_pending_tool_use(&bash_use("s1", "old_bash", "/home/u/projA", "git status", old_ts))
        .unwrap();
    assert!(l.resolve_tool_result(&result("s1", "old_bash", old_ts, 10)).unwrap());

    l.insert_pending_tool_use(&bash_use("s2", "recent_bash", "/home/u/projA", "ls", &recent_ts))
        .unwrap();
    assert!(l.resolve_tool_result(&result("s2", "recent_bash", &recent_ts, 20)).unwrap());

    let old_rtk = RtkCommandRow {
        rtk_row_id: 1,
        ts: old_ts.to_string(),
        project_path: "/home/u/projA".to_string(),
        head_hash: cmdnorm::hash(&cmdnorm::head("git status")),
        cmd_hash: cmdnorm::hash("git status"),
        raw_tokens: 100,
        filtered_tokens: 20,
        saved_tokens: 80,
        exec_time_ms: 5,
    };
    assert!(l.insert_rtk_event(&old_rtk).unwrap());

    let recent_rtk = RtkCommandRow {
        rtk_row_id: 2,
        ts: recent_ts.clone(),
        project_path: "/home/u/projA".to_string(),
        head_hash: cmdnorm::hash(&cmdnorm::head("ls")),
        cmd_hash: cmdnorm::hash("ls"),
        raw_tokens: 60,
        filtered_tokens: 20,
        saved_tokens: 40,
        exec_time_ms: 5,
    };
    assert!(l.insert_rtk_event(&recent_rtk).unwrap());

    let attribution = attribute(&l).unwrap();
    assert_eq!((attribution.text, attribution.none), (2, 0));

    // No filter: both rows count.
    assert_eq!(l.tool_aggregates(None, None).unwrap().iter().map(|r| r.calls).sum::<i64>(), 2);
    let all_rtk = l.rtk_totals(None, None).unwrap().unwrap();
    assert_eq!(all_rtk.commands, 2);
    assert_eq!(all_rtk.saved_tokens, 120);

    // days filter excludes the year-2000 row.
    let recent_tools = l.tool_aggregates(Some(7), None).unwrap();
    assert_eq!(recent_tools.len(), 1);
    assert_eq!(recent_tools[0].calls, 1);
    let recent_rtk_totals = l.rtk_totals(Some(7), None).unwrap().unwrap();
    assert_eq!(recent_rtk_totals.commands, 1);
    assert_eq!(recent_rtk_totals.saved_tokens, 40);

    // session filter isolates s1 regardless of age.
    let s1_tools = l.tool_aggregates(None, Some("s1")).unwrap();
    assert_eq!(s1_tools.len(), 1);
    assert_eq!(s1_tools[0].calls, 1);
    let s1_rtk = l.rtk_totals(None, Some("s1")).unwrap().unwrap();
    assert_eq!(s1_rtk.commands, 1);
    assert_eq!(s1_rtk.saved_tokens, 80);

    // days AND session compose: s1's only row is outside a 7-day window.
    assert!(l.tool_aggregates(Some(7), Some("s1")).unwrap().is_empty());
    assert!(l.rtk_totals(Some(7), Some("s1")).unwrap().is_none());
}

#[test]
fn scan_summary_json_omits_rtk_key_when_absent_and_includes_it_when_present() {
    let absent = runtab::ScanSummary::default();
    let json = serde_json::to_string(&absent).unwrap();
    assert!(!json.contains("\"rtk\""), "json was: {json}");

    let present = runtab::ScanSummary {
        rtk: Some(runtab::RtkReport {
            rows_imported: 1,
            attributed_text: 1,
            attributed_window: 0,
            unmatched: 0,
        }),
        ..Default::default()
    };
    let json = serde_json::to_string(&present).unwrap();
    assert!(json.contains("\"rtk\""), "json was: {json}");
}
