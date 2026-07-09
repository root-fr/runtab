//! Human-facing number formatting and terminal styling for CLI reports.
//! `--json` output never goes through this module.

use std::io::IsTerminal;

/// `13510551212` -> `13.5B`; one decimal, trailing `.0` trimmed, `<1000` plain.
pub fn fmt_tokens(n: i64) -> String {
    let abs = n.unsigned_abs();
    if abs < 1_000 {
        return n.to_string();
    }
    // Thresholds sit at 999_950 so a value that would round-format to
    // "1000.0K" rolls up to "1M" instead.
    let (value, unit) = if abs < 999_950 {
        (n as f64 / 1e3, "K")
    } else if abs < 999_950_000 {
        (n as f64 / 1e6, "M")
    } else {
        (n as f64 / 1e9, "B")
    };
    let s = format!("{value:.1}");
    let s = s.strip_suffix(".0").map(str::to_string).unwrap_or(s);
    format!("{s}{unit}")
}

/// `7636` -> `7,636`. For counts that must stay exact (events, calls).
pub fn fmt_count(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// `1456.8404` -> `$1,456.84`; positive sub-cent -> `<$0.01`; `None` -> `n/a`.
pub fn fmt_cost(cost: Option<f64>) -> String {
    let Some(c) = cost else {
        return "n/a".to_string();
    };
    // NaN/inf would leave `{:.2}` without a decimal point below.
    if !c.is_finite() {
        return "n/a".to_string();
    }
    if c > 0.0 && c < 0.005 {
        return "<$0.01".to_string();
    }
    let cents = format!("{:.2}", c.abs());
    let (whole, frac) = cents.split_once('.').expect("{:.2} always has a dot");
    let whole: i64 = whole.parse().unwrap_or(0);
    let sign = if c < 0.0 { "-" } else { "" };
    format!("{sign}${}.{frac}", fmt_count(whole))
}

/// `1 file`, `2 files`; the noun must pluralize with a plain trailing `s`.
pub fn fmt_noun(n: i64, noun: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{} {noun}{s}", fmt_count(n))
}

/// ANSI styling that no-ops when the terminal can't take it. Resolved once
/// and passed by reference so renderers stay pure and testable.
pub struct Style {
    enabled: bool,
}

impl Style {
    pub fn new(enabled: bool) -> Self {
        Style { enabled }
    }

    /// Colors on only for an interactive stdout with color left enabled.
    pub fn detect() -> Self {
        let enabled = std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var_os("TERM").is_none_or(|t| t != "dumb");
        Style::new(enabled)
    }

    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }

    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }

    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}
