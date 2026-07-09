use runtab::format::{fmt_cost, fmt_count, fmt_tokens, Style};

#[test]
fn fmt_tokens_humanizes_with_one_decimal_and_trims_point_zero() {
    assert_eq!(fmt_tokens(0), "0");
    assert_eq!(fmt_tokens(999), "999");
    assert_eq!(fmt_tokens(1_000), "1K");
    assert_eq!(fmt_tokens(355_082), "355.1K");
    assert_eq!(fmt_tokens(999_949), "999.9K");
    assert_eq!(fmt_tokens(999_950), "1M"); // rolls up instead of "1000.0K"
    assert_eq!(fmt_tokens(6_000_000), "6M");
    assert_eq!(fmt_tokens(6_670_786), "6.7M");
    assert_eq!(fmt_tokens(13_510_551_212), "13.5B");
}

#[test]
fn fmt_count_inserts_thousands_separators() {
    assert_eq!(fmt_count(0), "0");
    assert_eq!(fmt_count(999), "999");
    assert_eq!(fmt_count(7_636), "7,636");
    assert_eq!(fmt_count(13_510_551), "13,510,551");
}

#[test]
fn fmt_cost_two_decimals_separators_and_subcent_floor() {
    assert_eq!(fmt_cost(None), "n/a");
    assert_eq!(fmt_cost(Some(0.0)), "$0.00");
    assert_eq!(fmt_cost(Some(0.004)), "<$0.01");
    assert_eq!(fmt_cost(Some(3.4)), "$3.40");
    assert_eq!(fmt_cost(Some(1456.8404)), "$1,456.84");
    assert_eq!(fmt_cost(Some(13272.8308)), "$13,272.83");
}

#[test]
fn style_wraps_only_when_enabled() {
    let on = Style::new(true);
    let off = Style::new(false);
    assert_eq!(on.dim("x"), "\x1b[2mx\x1b[0m");
    assert_eq!(on.bold("x"), "\x1b[1mx\x1b[0m");
    assert_eq!(on.green("x"), "\x1b[32mx\x1b[0m");
    assert_eq!(on.yellow("x"), "\x1b[33mx\x1b[0m");
    assert_eq!(off.dim("x"), "x");
    assert_eq!(off.green("x"), "x");
}

#[test]
fn fmt_cost_non_finite_is_na() {
    assert_eq!(fmt_cost(Some(f64::NAN)), "n/a");
    assert_eq!(fmt_cost(Some(f64::INFINITY)), "n/a");
    assert_eq!(fmt_cost(Some(f64::NEG_INFINITY)), "n/a");
}

#[test]
fn fmt_noun_pluralizes() {
    use runtab::format::fmt_noun;
    assert_eq!(fmt_noun(1, "file"), "1 file");
    assert_eq!(fmt_noun(2, "file"), "2 files");
    assert_eq!(fmt_noun(0, "duplicate"), "0 duplicates");
    assert_eq!(fmt_noun(1_500, "new event"), "1,500 new events");
}
