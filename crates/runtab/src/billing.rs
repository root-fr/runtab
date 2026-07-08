//! Per-source billing-mode framing (spec "Cost framing"). A source that logs a
//! real `costUSD` (`cost_basis` = `logged`/`billed`) is metered/`api`; otherwise
//! it is a `subscription` whose dollars are labeled "API-equivalent value". The
//! user can override the auto-detected mode from the settings drawer.

pub const LABEL_SUBSCRIPTION: &str = "API-equivalent value";
pub const LABEL_API: &str = "estimated spend";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Subscription,
    Api,
    Mixed,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Subscription => "subscription",
            Mode::Api => "api",
            Mode::Mixed => "mixed",
        }
    }

    /// The recap-strip label for the whole selection. Mixed keeps the
    /// subscription framing at the top level; `modes[]` carries both.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Api => LABEL_API,
            _ => LABEL_SUBSCRIPTION,
        }
    }

    /// A 5h/weekly plan window only applies to subscription usage.
    pub fn plan_applicable(self) -> bool {
        self != Mode::Api
    }
}

/// Parse a stored/override mode string.
pub fn parse_override(s: Option<&str>) -> Option<Mode> {
    match s {
        Some("subscription") => Some(Mode::Subscription),
        Some("api") => Some(Mode::Api),
        _ => None,
    }
}

/// Resolve the effective mode for a selection from the override plus the count
/// of subscription-attributed and api-attributed events actually present.
pub fn resolve(override_mode: Option<Mode>, sub_events: i64, api_events: i64) -> Mode {
    match override_mode {
        Some(m) => m,
        None => {
            if api_events > 0 && sub_events > 0 {
                Mode::Mixed
            } else if api_events > 0 {
                Mode::Api
            } else {
                Mode::Subscription
            }
        }
    }
}
