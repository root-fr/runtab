use std::collections::BTreeSet;

use serde::Deserialize;

use crate::model::{CostBasis, UsageEvent};

const SNAPSHOT: &str = include_str!("pricing_snapshot.json");

#[derive(Debug, Deserialize)]
struct RateEntry {
    prefix: String,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write_5m: f64,
    cache_write_1h: f64,
}

#[derive(Debug, Deserialize)]
struct Snapshot {
    snapshot_date: String,
    models: Vec<RateEntry>,
}

/// Embedded pricing table. Model lookup is longest-prefix match so that a more
/// specific model id wins over a broader family entry when both are present.
pub struct Pricing {
    entries: Vec<RateEntry>,
    pub snapshot_date: String,
}

impl Pricing {
    pub fn load() -> Result<Pricing, serde_json::Error> {
        let mut snap: Snapshot = serde_json::from_str(SNAPSHOT)?;
        snap.models
            .sort_by_key(|r| std::cmp::Reverse(r.prefix.len()));
        Ok(Pricing {
            entries: snap.models,
            snapshot_date: snap.snapshot_date,
        })
    }

    fn rate_for(&self, model: &str) -> Option<&RateEntry> {
        self.entries.iter().find(|r| model.starts_with(r.prefix.as_str()))
    }

    fn cost_for(&self, e: &UsageEvent) -> Option<f64> {
        let r = self.rate_for(&e.model)?;
        let split_1h = e.cache_1h_tokens.max(0) as f64;
        let split_5m = e.cache_5m_tokens.max(0) as f64;
        let creation = e.cache_creation_tokens.max(0) as f64;
        // Cache-creation tokens not attributed to a named ephemeral tier are
        // priced at the 5m (default) write rate.
        let remainder = (creation - split_1h - split_5m).max(0.0);
        let cost = e.input_tokens as f64 * r.input
            + e.output_tokens as f64 * r.output
            + e.cache_read_tokens as f64 * r.cache_read
            + split_1h * r.cache_write_1h
            + (split_5m + remainder) * r.cache_write_5m;
        Some(cost)
    }

    /// Fill `cost_usd` from the snapshot. Unknown models leave cost NULL and are
    /// recorded in `unknown` for the scan summary — never guessed. Any
    /// adapter-provided figure — a logged real cost or a source-computed
    /// estimate — is better-informed than the embedded snapshot, so it is kept
    /// with its declared basis and never flags the model unknown.
    pub fn apply(&self, e: &mut UsageEvent, unknown: &mut BTreeSet<String>) {
        if e.cost_usd.is_some() {
            return;
        }
        e.cost_basis = CostBasis::Estimated;
        match self.cost_for(e) {
            Some(c) => e.cost_usd = Some(c),
            None => {
                e.cost_usd = None;
                unknown.insert(e.model.clone());
            }
        }
    }
}
