//! Attributes imported `rtk_events` rows to the Claude Code session that
//! actually ran them, by matching against `tool_events` (`tool_name =
//! 'Bash'`) with a completion `ts` within `WINDOW_SECS` of the rtk row's.
//! Three tiers, tried in order per row:
//!
//! 1. **`text`** — a candidate whose `bash_chain_hashes` contains the rtk
//!    row's `cmd_hash` (an exact normalized-command match on one chain
//!    segment). Multiple hits resolve to the nearest by `|Δts|`, ties to the
//!    lowest `tool_events.id` — a deterministic but arbitrary tie-break,
//!    since two truly simultaneous identical commands are indistinguishable
//!    from timing alone.
//! 2. **`window`** — no text hit. First try candidates whose
//!    `bash_head_hashes` contains the rtk row's `head_hash`, and take it
//!    *only if* every such candidate belongs to the same `(source,
//!    session_id)` — grouping on both fields together, not `session_id`
//!    alone, since two agents could reuse the same session id. This arm
//!    ignores `project_path`: a `cd` inside the Bash call can point rtk's
//!    cwd somewhere other than the session's project. Failing that, fall
//!    back to *every* Bash candidate in the window: if they all belong to
//!    one session AND that session's `project` equals the rtk row's
//!    `project_path`, attribute to the nearest one. Either path ties on the
//!    lowest id like tier 1.
//! 3. **`none`** — ambiguous (more than one candidate session) or no
//!    candidates at all. The row is left untouched (already `match_kind =
//!    'none'`) so a later run — with more transcript data scanned in the
//!    meantime — can retry it.
//!
//! Timestamps: `tool_events.ts` is transcript format
//! (`2026-07-07T19:10:20.902Z`); `rtk_events.ts` is rtk's own format
//! (`2026-07-07T19:11:16.498195575+00:00`). Both are parsed to whole-second
//! epoch via `timeutil::parse_rfc3339_to_epoch`, which only reads the
//! `YYYY-MM-DDTHH:MM:SS` prefix and ignores everything after — fraction and
//! offset suffix alike — so it already treats both shapes uniformly. The
//! ±120s window is enforced in Rust on that epoch, not in SQL: candidates
//! are first fetched with a `[-121s, +121s]` SQL prefilter (string-compared
//! against `tool_events.ts`, which carries fractional seconds — the extra
//! second absorbs the fact that e.g. `"...20.902Z" < "...20Z"` lexically)
//! and then filtered to the exact `|Δepoch| <= 120` in Rust. Comparing
//! epochs after a coarse SQL prefilter sidesteps every cross-format string
//! comparison edge case; only the 48h retry-horizon cutoff (see
//! `fetch_unattributed_rtk_rows`) is a bare string comparison, which is
//! sound at whole-second granularity since rtk's ts prefix has the same
//! `YYYY-MM-DDT...` shape as the bound `epoch_to_rfc3339` produces.

use std::collections::HashSet;

use serde::Serialize;

use crate::ledger::Ledger;
use crate::model::{BashCandidate, UnattributedRtkRow};
use crate::timeutil::{epoch_to_rfc3339, now_epoch, parse_rfc3339_to_epoch};

const WINDOW_SECS: i64 = 120;
/// Extra margin on the SQL prefilter so whole-second string comparison
/// against a fractional-second `ts` never excludes a genuine boundary
/// candidate; see the module doc comment.
const PREFILTER_SLOP_SECS: i64 = 1;
const RETRY_HORIZON_SECS: i64 = 48 * 3600;
/// Rows per write transaction, mirroring `rtkimport::apply_batch`'s tx
/// pattern: bulk `UPDATE`s under one commit instead of autocommit-per-row,
/// without holding a single multi-minute transaction open on a large backfill.
const CHUNK_SIZE: usize = 500;
/// Above this many pending rows, a run is slow enough (measured ~50s on a
/// ~90k-row first import) to warrant progress output; below it, stay silent.
const PROGRESS_THRESHOLD: usize = 5000;

/// Outcome of one `attribute` run, counted by tier.
#[derive(Debug, Default, Serialize)]
pub struct AttributionSummary {
    pub text: u64,
    pub window: u64,
    pub none: u64,
}

/// A `Bash` `tool_events` row, decoded into the shape `resolve` matches
/// against: JSON hash arrays parsed and `ts` reduced to epoch.
struct Candidate {
    id: i64,
    source: String,
    session_id: String,
    project: String,
    ts_epoch: i64,
    head_hashes: Vec<String>,
    chain_hashes: Vec<String>,
}

impl Candidate {
    /// `None` if `ts` fails to parse — never observed in practice (`ts`
    /// comes from `resolve_tool_result`'s own transcript timestamp), but a
    /// candidate we can't place in time can't be matched against, so it's
    /// dropped rather than crashing the whole run.
    fn from_row(row: BashCandidate) -> Option<Candidate> {
        let ts_epoch = parse_rfc3339_to_epoch(&row.ts)?;
        Some(Candidate {
            id: row.id,
            source: row.source,
            session_id: row.session_id,
            project: row.project,
            ts_epoch,
            head_hashes: parse_hash_array(row.bash_head_hashes.as_deref()),
            chain_hashes: parse_hash_array(row.bash_chain_hashes.as_deref()),
        })
    }
}

/// A malformed or absent JSON array is treated as empty, never an error —
/// it just means this candidate can't hash-match, not that the run fails.
fn parse_hash_array(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok()).unwrap_or_default()
}

enum Match<'a> {
    Text(&'a Candidate),
    Window(&'a Candidate),
}

/// Process every unattributed rtk row and upgrade `match_kind` where a
/// match is found. Rows are picked up either because they're past the
/// persisted attribution watermark (`rtk_events.id > last_attributed_rtk_id`
/// — never yet examined, however old, so an interrupted backfill resumes
/// instead of leaving a permanent gap) or because they're recent enough that
/// a late-arriving transcript might still explain them (within
/// `RETRY_HORIZON_SECS` of now). Only `match_kind = 'none'` rows are ever
/// read, so a match is never downgraded and re-running is idempotent. The
/// watermark advances chunk-by-chunk as rows are examined (see
/// `attribute_chunk`), so a run interrupted partway still checkpoints its
/// progress.
pub fn attribute(ledger: &Ledger) -> anyhow::Result<AttributionSummary> {
    let watermark = ledger.attribution_watermark()?;
    let cutoff = epoch_to_rfc3339(now_epoch() - RETRY_HORIZON_SECS);
    let rows = ledger.fetch_unattributed_rtk_rows(watermark, &cutoff)?;

    let show_progress = rows.len() > PROGRESS_THRESHOLD;
    if show_progress {
        eprintln!("runtab: attributing {} rtk commands...", rows.len());
    }
    let started = std::time::Instant::now();

    let mut summary = AttributionSummary::default();
    for chunk in rows.chunks(CHUNK_SIZE) {
        let chunk_summary = attribute_chunk(ledger, chunk)?;
        summary.text += chunk_summary.text;
        summary.window += chunk_summary.window;
        summary.none += chunk_summary.none;
    }

    if show_progress {
        eprintln!(
            "runtab: attributed {} rtk commands in {:.1}s",
            rows.len(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(summary)
}

/// Attributes one chunk inside its own transaction, mirroring
/// `rtkimport::apply_batch`: any error mid-chunk rolls the whole chunk back
/// rather than leaving a half-attributed batch, and the read (candidate
/// lookup) and write (attribution `UPDATE`) both run on the same connection
/// inside that transaction without issue. The watermark advances to this
/// chunk's highest `id` (rows arrive ordered ascending, so that's the last
/// one) in the same commit as its attribution updates — a crash between
/// chunks leaves the watermark at the last fully-committed chunk, never
/// ahead of it.
fn attribute_chunk(ledger: &Ledger, chunk: &[UnattributedRtkRow]) -> anyhow::Result<AttributionSummary> {
    ledger.tx_begin()?;
    let mut summary = AttributionSummary::default();
    for row in chunk {
        match attribute_one(ledger, row) {
            Ok(Tier::Text) => summary.text += 1,
            Ok(Tier::Window) => summary.window += 1,
            Ok(Tier::None) => summary.none += 1,
            Err(e) => {
                let _ = ledger.tx_rollback();
                return Err(e);
            }
        }
    }
    if let Some(last) = chunk.last() {
        if let Err(e) = ledger.set_attribution_watermark(last.id) {
            let _ = ledger.tx_rollback();
            return Err(e.into());
        }
    }
    if let Err(e) = ledger.tx_commit() {
        let _ = ledger.tx_rollback();
        return Err(e.into());
    }
    Ok(summary)
}

enum Tier {
    Text,
    Window,
    None,
}

fn attribute_one(ledger: &Ledger, row: &UnattributedRtkRow) -> anyhow::Result<Tier> {
    let Some(row_epoch) = parse_rfc3339_to_epoch(&row.ts) else {
        return Ok(Tier::None);
    };

    let lower = epoch_to_rfc3339(row_epoch - WINDOW_SECS - PREFILTER_SLOP_SECS);
    let upper = epoch_to_rfc3339(row_epoch + WINDOW_SECS + PREFILTER_SLOP_SECS);
    let candidates: Vec<Candidate> = ledger
        .bash_candidates_in_range(&lower, &upper)?
        .into_iter()
        .filter_map(Candidate::from_row)
        .filter(|c| (c.ts_epoch - row_epoch).abs() <= WINDOW_SECS)
        .collect();

    match resolve(row, row_epoch, &candidates) {
        Some(Match::Text(c)) => {
            ledger.update_rtk_attribution(row.id, &c.source, &c.session_id, c.id, "text")?;
            Ok(Tier::Text)
        }
        Some(Match::Window(c)) => {
            ledger.update_rtk_attribution(row.id, &c.source, &c.session_id, c.id, "window")?;
            Ok(Tier::Window)
        }
        None => Ok(Tier::None),
    }
}

fn resolve<'a>(row: &UnattributedRtkRow, row_epoch: i64, candidates: &'a [Candidate]) -> Option<Match<'a>> {
    let text_hits: Vec<&Candidate> =
        candidates.iter().filter(|c| c.chain_hashes.iter().any(|h| h == &row.cmd_hash)).collect();
    if let Some(c) = nearest(&text_hits, row_epoch) {
        return Some(Match::Text(c));
    }

    let head_hits: Vec<&Candidate> =
        candidates.iter().filter(|c| c.head_hashes.iter().any(|h| h == &row.head_hash)).collect();
    if only_one_session(&head_hits) {
        if let Some(c) = nearest(&head_hits, row_epoch) {
            return Some(Match::Window(c));
        }
    }

    let all: Vec<&Candidate> = candidates.iter().collect();
    if only_one_session(&all) && all[0].project == row.project_path {
        if let Some(c) = nearest(&all, row_epoch) {
            return Some(Match::Window(c));
        }
    }

    None
}

/// Whether every candidate belongs to the same `(source, session_id)` pair
/// — grouped on both fields together (see module doc comment) — and there
/// is at least one candidate to begin with.
fn only_one_session(candidates: &[&Candidate]) -> bool {
    let sessions: HashSet<(&str, &str)> =
        candidates.iter().map(|c| (c.source.as_str(), c.session_id.as_str())).collect();
    sessions.len() == 1
}

fn nearest<'a>(candidates: &[&'a Candidate], target_epoch: i64) -> Option<&'a Candidate> {
    candidates.iter().min_by_key(|c| ((c.ts_epoch - target_epoch).abs(), c.id)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmdnorm;
    use crate::model::{RtkCommandRow, ToolResultSeen, ToolUseSeen};

    /// Arbitrary fixed epoch so tests 1-5 and 8 don't depend on wall-clock
    /// time; tests 6 and 7 (retry-horizon behavior) use real `now_epoch()`.
    const BASE: i64 = 1_800_000_000;

    fn rtk_ts(epoch: i64) -> String {
        format!("{}.987654321+00:00", &epoch_to_rfc3339(epoch)[..19])
    }

    fn cc_ts(epoch: i64) -> String {
        format!("{}.123Z", &epoch_to_rfc3339(epoch)[..19])
    }

    fn bash_tool_use(source: &str, session: &str, tool_use_id: &str, project: &str, chain_cmd: &str) -> ToolUseSeen {
        ToolUseSeen {
            source: source.to_string(),
            session_id: session.to_string(),
            tool_use_id: tool_use_id.to_string(),
            ts: "2026-01-01T00:00:00Z".to_string(),
            project: project.to_string(),
            tool_name: "Bash".to_string(),
            est_args_tokens: 3,
            bash_head_hashes: Some(serde_json::to_string(&cmdnorm::chain_head_hashes(chain_cmd)).unwrap()),
            bash_chain_hashes: Some(serde_json::to_string(&cmdnorm::chain_hashes(chain_cmd)).unwrap()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_bash_event_from(
        l: &Ledger,
        source: &str,
        session: &str,
        tool_use_id: &str,
        project: &str,
        chain_cmd: &str,
        completion_ts: &str,
    ) {
        l.insert_pending_tool_use(&bash_tool_use(source, session, tool_use_id, project, chain_cmd)).unwrap();
        let result = ToolResultSeen {
            source: source.to_string(),
            session_id: session.to_string(),
            tool_use_id: tool_use_id.to_string(),
            ts: completion_ts.to_string(),
            est_result_tokens: 7,
            is_error: false,
        };
        assert!(l.resolve_tool_result(&result).unwrap());
    }

    fn insert_bash_event(l: &Ledger, session: &str, tool_use_id: &str, project: &str, chain_cmd: &str, completion_ts: &str) {
        insert_bash_event_from(l, "claude_code", session, tool_use_id, project, chain_cmd, completion_ts);
    }

    fn insert_rtk_row(l: &Ledger, rtk_row_id: i64, ts: &str, project_path: &str, cmd: &str) {
        let row = RtkCommandRow {
            rtk_row_id,
            ts: ts.to_string(),
            project_path: project_path.to_string(),
            head_hash: cmdnorm::hash(&cmdnorm::head(cmd)),
            cmd_hash: cmdnorm::hash(cmd),
            raw_tokens: 500,
            filtered_tokens: 100,
            saved_tokens: 400,
            exec_time_ms: 12,
        };
        assert!(l.insert_rtk_event(&row).unwrap());
    }

    #[test]
    fn text_match_wins_over_closer_window_only_match() {
        let l = Ledger::open_in_memory().unwrap();
        insert_bash_event(&l, "s-far", "toolu_far", "/home/u/p", "git status", &cc_ts(BASE + 90));
        insert_bash_event(&l, "s-near", "toolu_near", "/home/u/p", "git log", &cc_ts(BASE + 5));
        insert_rtk_row(&l, 1, &rtk_ts(BASE), "/home/u/p", "git status");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (1, 0, 0));

        let (_, source, session_id, tool_event_id, match_kind) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(match_kind, "text");
        assert_eq!(source.as_deref(), Some("claude_code"));
        assert_eq!(session_id.as_deref(), Some("s-far"));
        assert_eq!(tool_event_id, l.tool_event_id("claude_code", "s-far", "toolu_far").unwrap());
    }

    #[test]
    fn two_identical_commands_different_sessions_attribute_to_nearest_no_crosstalk() {
        let l = Ledger::open_in_memory().unwrap();
        insert_bash_event(&l, "s1", "toolu_1", "/home/u/p", "cargo test", &cc_ts(BASE));
        insert_bash_event(&l, "s2", "toolu_2", "/home/u/p", "cargo test", &cc_ts(BASE + 30));

        insert_rtk_row(&l, 1, &rtk_ts(BASE + 1), "/home/u/p", "cargo test");
        insert_rtk_row(&l, 2, &rtk_ts(BASE + 29), "/home/u/p", "cargo test");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (2, 0, 0));

        let (_, _, session1, tool_event1, kind1) = l.rtk_event_state(1).unwrap().unwrap();
        let (_, _, session2, tool_event2, kind2) = l.rtk_event_state(2).unwrap().unwrap();
        assert_eq!(kind1, "text");
        assert_eq!(kind2, "text");
        assert_eq!(session1.as_deref(), Some("s1"));
        assert_eq!(session2.as_deref(), Some("s2"));
        assert_eq!(tool_event1, l.tool_event_id("claude_code", "s1", "toolu_1").unwrap());
        assert_eq!(tool_event2, l.tool_event_id("claude_code", "s2", "toolu_2").unwrap());
        assert_ne!(tool_event1, tool_event2);
    }

    #[test]
    fn ambiguous_multiple_sessions_no_hash_match_stays_none() {
        let l = Ledger::open_in_memory().unwrap();
        insert_bash_event(&l, "s1", "toolu_1", "/proj/B", "npm test", &cc_ts(BASE));
        insert_bash_event(&l, "s2", "toolu_2", "/proj/C", "npm test", &cc_ts(BASE + 10));
        insert_rtk_row(&l, 1, &rtk_ts(BASE), "/proj/A", "unique-cmd-xyz");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (0, 0, 1));

        let (_, source, session_id, tool_event_id, match_kind) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(match_kind, "none");
        assert!(source.is_none());
        assert!(session_id.is_none());
        assert!(tool_event_id.is_none());
    }

    #[test]
    fn single_session_in_window_with_matching_project_attributes_via_window() {
        let l = Ledger::open_in_memory().unwrap();
        insert_bash_event(&l, "s1", "toolu_1", "/proj/A", "npm test", &cc_ts(BASE + 15));
        insert_rtk_row(&l, 1, &rtk_ts(BASE), "/proj/A", "unique-cmd-abc");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (0, 1, 0));

        let (_, source, session_id, tool_event_id, match_kind) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(match_kind, "window");
        assert_eq!(source.as_deref(), Some("claude_code"));
        assert_eq!(session_id.as_deref(), Some("s1"));
        assert_eq!(tool_event_id, l.tool_event_id("claude_code", "s1", "toolu_1").unwrap());
    }

    #[test]
    fn cross_project_head_hash_arm_ignores_project_mismatch() {
        let l = Ledger::open_in_memory().unwrap();
        insert_bash_event(&l, "s-cd", "toolu_cd", "/home/u/myproj", "cd /elsewhere && git status", &cc_ts(BASE + 3));

        // A different exact git invocation than the transcript captured
        // (e.g. a shell alias/flag), sharing only the "git" head — proves
        // this arm doesn't need a text hit.
        insert_rtk_row(&l, 1, &rtk_ts(BASE), "/elsewhere", "git status --porcelain=v2");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (0, 1, 0));

        let (_, source, session_id, tool_event_id, match_kind) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(match_kind, "window");
        assert_eq!(source.as_deref(), Some("claude_code"));
        assert_eq!(session_id.as_deref(), Some("s-cd"));
        assert_eq!(tool_event_id, l.tool_event_id("claude_code", "s-cd", "toolu_cd").unwrap());
    }

    #[test]
    fn two_sessions_with_head_hash_hits_stays_ambiguous_none() {
        let l = Ledger::open_in_memory().unwrap();
        insert_bash_event(&l, "s1", "toolu_1", "/home/u/p", "git log", &cc_ts(BASE + 5));
        insert_bash_event(&l, "s2", "toolu_2", "/home/u/p", "git diff", &cc_ts(BASE - 5));
        insert_rtk_row(&l, 1, &rtk_ts(BASE), "/home/u/p", "git status --unmatched-flag");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (0, 0, 1));

        let (_, source, session_id, tool_event_id, match_kind) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(match_kind, "none");
        assert!(source.is_none());
        assert!(session_id.is_none());
        assert!(tool_event_id.is_none());
    }

    #[test]
    fn grouping_uses_both_source_and_session_id_not_session_id_alone() {
        let l = Ledger::open_in_memory().unwrap();
        // Same session_id string reused across two different sources:
        // grouping on session_id alone would wrongly see one session here.
        insert_bash_event_from(&l, "codex", "shared-id", "toolu_1", "/home/u/p", "git log", &cc_ts(BASE + 5));
        insert_bash_event_from(&l, "claude_code", "shared-id", "toolu_2", "/home/u/p", "git diff", &cc_ts(BASE - 5));
        insert_rtk_row(&l, 1, &rtk_ts(BASE), "/home/u/p", "git status --unmatched-flag2");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (0, 0, 1));
        let (_, _, _, _, match_kind) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(match_kind, "none");
    }

    #[test]
    fn window_boundary_is_inclusive_at_exactly_120_seconds() {
        let l = Ledger::open_in_memory().unwrap();
        insert_bash_event(&l, "s1", "toolu_1", "/home/u/p", "git status", &cc_ts(BASE + WINDOW_SECS));
        insert_rtk_row(&l, 1, &rtk_ts(BASE), "/home/u/p", "git status");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (1, 0, 0));
    }

    #[test]
    fn candidate_just_outside_the_120_second_window_is_excluded() {
        let l = Ledger::open_in_memory().unwrap();
        insert_bash_event(&l, "s1", "toolu_1", "/home/u/p", "git status", &cc_ts(BASE + WINDOW_SECS + 1));
        insert_rtk_row(&l, 1, &rtk_ts(BASE), "/home/u/p", "git status");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (0, 0, 1));
    }

    #[test]
    fn late_transcript_upgrades_to_text_on_rerun_then_stays_idempotent() {
        let l = Ledger::open_in_memory().unwrap();
        let rtk_epoch = now_epoch() - 30; // recent: within the 48h retry horizon

        insert_rtk_row(&l, 1, &rtk_ts(rtk_epoch), "/home/u/p", "git status");

        let first = attribute(&l).unwrap();
        assert_eq!((first.text, first.window, first.none), (0, 0, 1));
        let (_, _, _, _, kind_after_first) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(kind_after_first, "none");

        // The transcript for this command lands after the first pass.
        insert_bash_event(&l, "s1", "toolu_late", "/home/u/p", "git status", &cc_ts(rtk_epoch + 2));

        let second = attribute(&l).unwrap();
        assert_eq!((second.text, second.window, second.none), (1, 0, 0));
        let (_, source, session_id, tool_event_id, kind_after_second) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(kind_after_second, "text");
        assert_eq!(source.as_deref(), Some("claude_code"));
        assert_eq!(session_id.as_deref(), Some("s1"));
        assert_eq!(tool_event_id, l.tool_event_id("claude_code", "s1", "toolu_late").unwrap());

        let third = attribute(&l).unwrap();
        assert_eq!((third.text, third.window, third.none), (0, 0, 0));
        let (_, _, session_id3, tool_event_id3, kind_after_third) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(kind_after_third, "text");
        assert_eq!(session_id3, session_id);
        assert_eq!(tool_event_id3, tool_event_id);
    }

    #[test]
    fn watermark_lets_a_row_older_than_the_retry_horizon_be_processed_once() {
        let l = Ledger::open_in_memory().unwrap();
        let old_epoch = now_epoch() - 50 * 3600; // 50h ago: past the 48h retry horizon

        insert_rtk_row(&l, 1, &rtk_ts(old_epoch), "/home/u/p", "git status");
        insert_bash_event(&l, "s1", "toolu_1", "/home/u/p", "git status", &cc_ts(old_epoch + 1));

        assert_eq!(l.attribution_watermark().unwrap(), 0);

        // The row's id (1) is above the fresh-ledger watermark (0), so it's
        // examined despite being past the retry horizon.
        let first = attribute(&l).unwrap();
        assert_eq!((first.text, first.window, first.none), (1, 0, 0));
        assert_eq!(l.attribution_watermark().unwrap(), 1);

        // Re-running finds nothing left below or above the watermark.
        let second = attribute(&l).unwrap();
        assert_eq!((second.text, second.window, second.none), (0, 0, 0));
        assert_eq!(l.attribution_watermark().unwrap(), 1);
    }

    #[test]
    fn watermark_resumes_an_interrupted_backfill_instead_of_leaving_a_gap() {
        let l = Ledger::open_in_memory().unwrap();
        let old_epoch = now_epoch() - 50 * 3600; // 50h ago: past the 48h retry horizon

        // First tranche, as if one chunk of a backfill committed before a crash.
        insert_rtk_row(&l, 1, &rtk_ts(old_epoch), "/home/u/p", "git status");
        insert_bash_event(&l, "s1", "toolu_1", "/home/u/p", "git status", &cc_ts(old_epoch + 1));

        let first = attribute(&l).unwrap();
        assert_eq!((first.text, first.window, first.none), (1, 0, 0));
        assert_eq!(l.attribution_watermark().unwrap(), 1);

        // More rows land with higher ids but older-than-horizon timestamps —
        // imported by the same backfill, but never attempted before the crash.
        insert_rtk_row(&l, 2, &rtk_ts(old_epoch), "/home/u/p", "cargo test");
        insert_bash_event(&l, "s2", "toolu_2", "/home/u/p", "cargo test", &cc_ts(old_epoch + 1));

        let second = attribute(&l).unwrap();
        assert_eq!((second.text, second.window, second.none), (1, 0, 0));
        assert_eq!(l.attribution_watermark().unwrap(), 2);
        let (_, _, session_id, _, kind) = l.rtk_event_state(2).unwrap().unwrap();
        assert_eq!(kind, "text");
        assert_eq!(session_id.as_deref(), Some("s2"));
    }

    #[test]
    fn cross_format_timestamps_five_seconds_apart_still_match() {
        let l = Ledger::open_in_memory().unwrap();
        // rtk-format ts (9-digit fraction, explicit +00:00 offset) vs
        // transcript-format ts (3-digit millis, Z suffix), 5s apart —
        // proves the epoch parser and window math cross formats.
        insert_bash_event(&l, "s1", "toolu_1", "/home/u/p", "pytest -q", &cc_ts(BASE + 5));
        insert_rtk_row(&l, 1, &rtk_ts(BASE), "/home/u/p", "pytest -q");

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (1, 0, 0));

        let (_, source, session_id, tool_event_id, match_kind) = l.rtk_event_state(1).unwrap().unwrap();
        assert_eq!(match_kind, "text");
        assert_eq!(source.as_deref(), Some("claude_code"));
        assert_eq!(session_id.as_deref(), Some("s1"));
        assert_eq!(tool_event_id, l.tool_event_id("claude_code", "s1", "toolu_1").unwrap());
    }

    #[test]
    fn chunking_attributes_every_row_across_the_500_row_boundary() {
        let l = Ledger::open_in_memory().unwrap();
        const N: i64 = 501;
        for i in 0..N {
            let epoch = BASE + i * 1000; // spaced far apart: no cross-row window overlap
            let session = format!("s{i}");
            let tool_use_id = format!("toolu_{i}");
            let cmd = format!("unique-cmd-{i}");
            insert_bash_event(&l, &session, &tool_use_id, "/home/u/p", &cmd, &cc_ts(epoch));
            insert_rtk_row(&l, i + 1, &rtk_ts(epoch), "/home/u/p", &cmd);
        }

        let summary = attribute(&l).unwrap();
        assert_eq!((summary.text, summary.window, summary.none), (N as u64, 0, 0));

        for i in 0..N {
            let (_, _, _, _, match_kind) = l.rtk_event_state(i + 1).unwrap().unwrap();
            assert_eq!(match_kind, "text");
        }
    }
}
