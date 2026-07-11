use runtab::ledger::Ledger;
use runtab::model::{CostBasis, UsageEvent};

/// Build a minimal usage event (input tokens only) with a controllable
/// timestamp and cost basis, for driving the ledger directly in tests.
#[allow(clippy::too_many_arguments)]
pub fn ev(
    msg: &str,
    session: &str,
    project: &str,
    model: &str,
    ts: &str,
    input: i64,
    basis: CostBasis,
    cost: Option<f64>,
) -> UsageEvent {
    UsageEvent {
        source: "claude_code".to_string(),
        message_id: msg.to_string(),
        request_id: format!("r-{msg}"),
        session_id: session.to_string(),
        ts: ts.to_string(),
        model: model.to_string(),
        input_tokens: input,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        cache_1h_tokens: 0,
        cache_5m_tokens: 0,
        reasoning_tokens: 0,
        project: project.to_string(),
        agent_version: "1.0.0".to_string(),
        cost_usd: cost,
        cost_basis: basis,
    }
}

/// Like `ev`, but with a controllable `source` (agent id in local underscore
/// form, e.g. `"claude_code"`/`"codex"`) so tests can exercise the agent filter
/// and the `/api/agents` rollup across sources.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub fn ev_src(
    source: &str,
    msg: &str,
    session: &str,
    project: &str,
    model: &str,
    ts: &str,
    input: i64,
) -> UsageEvent {
    UsageEvent {
        source: source.to_string(),
        ..ev(msg, session, project, model, ts, input, CostBasis::Estimated, None)
    }
}

pub fn insert(l: &Ledger, e: &UsageEvent) {
    l.upsert(e).expect("upsert");
}

/// Complete the pre-sync review by accepting every project as-is, opening the
/// consent gate so `push_all` may send. Mirrors what `runtab sync login` records
/// on empty input. (Not every test binary that shares this module uses it.)
#[allow(dead_code)]
pub fn review_all(l: &Ledger) {
    let items = l.project_review_items().expect("review items");
    l.set_project_review(&items).expect("set review");
}
