use std::collections::BTreeSet;

use runtab::model::{CostBasis, UsageEvent};
use runtab::pricing::Pricing;

fn base_event(model: &str) -> UsageEvent {
    UsageEvent {
        source: "claude_code".to_string(),
        message_id: "m".to_string(),
        request_id: "r".to_string(),
        session_id: "s".to_string(),
        ts: "2026-07-06T00:00:00.000Z".to_string(),
        model: model.to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        cache_1h_tokens: 0,
        cache_5m_tokens: 0,
        reasoning_tokens: 0,
        project: "p".to_string(),
        agent_version: "1.0.0".to_string(),
        cost_usd: None,
        cost_basis: CostBasis::Estimated,
    }
}

#[test]
fn simple_input_output_cost() {
    let pricing = Pricing::load().unwrap();
    let mut e = base_event("claude-haiku-4-5-20251001");
    e.input_tokens = 1_000_000;
    e.output_tokens = 1_000_000;

    let mut unknown = BTreeSet::new();
    pricing.apply(&mut e, &mut unknown);

    // Haiku: $1.00 input + $5.00 output per 1M = $6.00
    let cost = e.cost_usd.unwrap();
    assert!((cost - 6.0).abs() < 1e-6, "cost was {cost}");
    assert_eq!(e.cost_basis, CostBasis::Estimated);
    assert!(unknown.is_empty());
}

#[test]
fn cache_split_pricing_prices_each_tier() {
    let pricing = Pricing::load().unwrap();
    let mut e = base_event("claude-opus-4-8");
    e.input_tokens = 20;
    e.output_tokens = 100;
    e.cache_read_tokens = 5000;
    e.cache_creation_tokens = 1000;
    e.cache_1h_tokens = 400;
    e.cache_5m_tokens = 600;

    let mut unknown = BTreeSet::new();
    pricing.apply(&mut e, &mut unknown);

    // 20*5e-6 + 100*25e-6 + 5000*5e-7 + 400*1e-5 + 600*6.25e-6 = 0.01285
    let cost = e.cost_usd.unwrap();
    assert!((cost - 0.01285).abs() < 1e-9, "cost was {cost}");
}

#[test]
fn cache_creation_without_splits_uses_5m_rate() {
    let pricing = Pricing::load().unwrap();
    let mut e = base_event("claude-opus-4-8");
    e.cache_creation_tokens = 1000; // no 1h/5m split reported

    let mut unknown = BTreeSet::new();
    pricing.apply(&mut e, &mut unknown);

    // 1000 * 6.25e-6 = 0.00625
    let cost = e.cost_usd.unwrap();
    assert!((cost - 0.00625).abs() < 1e-9, "cost was {cost}");
}

#[test]
fn legacy_opus_priced_above_the_4_5_plus_family() {
    let pricing = Pricing::load().unwrap();
    let mut unknown = BTreeSet::new();

    // Opus 4.0 and 4.1 list at $15/$75, not the $5/$25 of Opus 4.5-4.8.
    for id in ["claude-opus-4-1-20250805", "claude-opus-4-20250514"] {
        let mut e = base_event(id);
        e.input_tokens = 1_000_000;
        e.output_tokens = 1_000_000;
        pricing.apply(&mut e, &mut unknown);
        let cost = e.cost_usd.unwrap();
        assert!((cost - 90.0).abs() < 1e-6, "{id} cost was {cost}");
    }
    assert!(unknown.is_empty());
}

#[test]
fn opus_4_8_keeps_the_current_family_rate() {
    let pricing = Pricing::load().unwrap();
    let mut e = base_event("claude-opus-4-8");
    e.input_tokens = 1_000_000;
    e.output_tokens = 1_000_000;

    let mut unknown = BTreeSet::new();
    pricing.apply(&mut e, &mut unknown);

    // $5 input + $25 output per 1M = $30.
    let cost = e.cost_usd.unwrap();
    assert!((cost - 30.0).abs() < 1e-6, "cost was {cost}");
}

#[test]
fn unknown_model_is_null_and_collected() {
    let pricing = Pricing::load().unwrap();
    let mut e = base_event("mystery-model-1");
    e.input_tokens = 100;

    let mut unknown = BTreeSet::new();
    pricing.apply(&mut e, &mut unknown);

    assert!(e.cost_usd.is_none());
    assert!(unknown.contains("mystery-model-1"));
}
