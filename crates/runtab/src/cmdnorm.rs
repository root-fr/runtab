//! Normalizes Bash command text so a Claude Code transcript's command and
//! rtk's ledger `original_cmd` hash equal when they refer to the same
//! invocation. rtk's ledger stores each chain segment separately,
//! shell-dequoted, without the `rtk` prefix — this module reproduces that
//! shape from raw transcript text so both sides can be compared by hash
//! alone (raw command text is never stored).

use crate::encoding;

/// Split `s` on top-level `&&`, `||`, `;`, `|`, and newline. A minimal state
/// machine tracks single/double quotes so operators inside them don't split
/// the chain — not a full shell parser (no backslash escaping, no
/// subshells). Segments are trimmed; empty segments (e.g. a trailing `;`)
/// are dropped. Quoting is preserved verbatim; dequoting happens in
/// `normalize`, not here.
pub fn split_chain(s: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if in_single {
            current.push(c);
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            current.push(c);
            if c == '"' {
                in_double = false;
            }
            continue;
        }
        match c {
            '\'' => {
                in_single = true;
                current.push(c);
            }
            '"' => {
                in_double = true;
                current.push(c);
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                segments.push(current.trim().to_string());
                current = String::new();
            }
            '|' if chars.peek() == Some(&'|') => {
                chars.next();
                segments.push(current.trim().to_string());
                current = String::new();
            }
            ';' | '|' | '\n' => {
                segments.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(c),
        }
    }
    segments.push(current.trim().to_string());
    segments.retain(|seg| !seg.is_empty());
    segments
}

/// Shell-style tokenize (dequote, collapse whitespace), drop a leading `rtk`
/// token and any leading `NAME=value` env assignments, then rejoin with
/// single spaces. Leading `rtk` and env-assignment tokens are stripped in
/// whatever order they appear, repeatedly, so both `rtk FOO=1 cmd` and
/// `FOO=1 rtk cmd` normalize the same; a `NAME=value`-shaped token deeper in
/// the command (e.g. `git commit -m X=Y`) is left alone.
///
/// `s` must be a single chain segment (see `split_chain`), not raw chain
/// text — the tokenizer has no notion of `&&`/`||`/`;`/`|`, so e.g.
/// `"FOO=1;ls"` tokenizes as one garbage token. For raw command text, split
/// first and normalize each segment, or use `chain_hashes`.
pub fn normalize(s: &str) -> String {
    let mut tokens = tokenize(s);
    loop {
        match tokens.first() {
            Some(t) if t == "rtk" => {
                tokens.remove(0);
            }
            Some(t) if is_env_assignment(t) => {
                tokens.remove(0);
            }
            _ => break,
        }
    }
    tokens.join(" ")
}

/// First normalized token, basename if it contains `/`. Empty/whitespace
/// input returns `""`.
///
/// `s` must be a single chain segment (see `split_chain`), same contract as
/// `normalize`; raw chain text (e.g. `"FOO=1;ls"`) does not split here and
/// produces a garbage head. For raw command text, use `chain_head_hashes`.
pub fn head(s: &str) -> String {
    let normalized = normalize(s);
    let Some(first) = normalized.split(' ').next().filter(|t| !t.is_empty()) else {
        return String::new();
    };
    match first.rsplit('/').next() {
        Some(base) => base.to_string(),
        None => first.to_string(),
    }
}

/// SHA-256 hex of the normalized command.
///
/// `s` must be a single chain segment (see `split_chain`), same contract as
/// `normalize`. For raw command text, use `chain_hashes`.
pub fn hash(s: &str) -> String {
    encoding::sha256_hex(&[&normalize(s)])
}

/// Hash of each top-level chain segment, in order.
pub fn chain_hashes(s: &str) -> Vec<String> {
    split_chain(s).iter().map(|seg| hash(seg)).collect()
}

/// Hash of each top-level chain segment's head, in order, parallel to
/// `chain_hashes`. Segments whose head is empty (env-assignment-only, e.g.
/// `FOO=1` with no command) are skipped rather than hashed, so they don't
/// collide on a shared degenerate `hash("")`.
pub fn chain_head_hashes(s: &str) -> Vec<String> {
    split_chain(s)
        .iter()
        .map(|seg| head(seg))
        .filter(|h| !h.is_empty())
        .map(|h| hash(&h))
        .collect()
}

/// Splits `s` into whitespace-separated tokens, treating single- and
/// double-quoted spans as part of the current token and stripping their
/// quotes. No backslash escaping. An unclosed quote is not an error: the
/// rest of the string becomes part of that token.
fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_current = false;
    let mut chars = s.chars();

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has_current = true;
                for c2 in chars.by_ref() {
                    if c2 == '\'' {
                        break;
                    }
                    current.push(c2);
                }
            }
            '"' => {
                has_current = true;
                for c2 in chars.by_ref() {
                    if c2 == '"' {
                        break;
                    }
                    current.push(c2);
                }
            }
            c if c.is_whitespace() => {
                if has_current {
                    tokens.push(std::mem::take(&mut current));
                    has_current = false;
                }
            }
            c => {
                has_current = true;
                current.push(c);
            }
        }
    }
    if has_current {
        tokens.push(current);
    }
    tokens
}

/// Whether `token` is a leading env-var assignment: `[A-Za-z_][A-Za-z0-9_]*=...`.
fn is_env_assignment(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    for c in chars {
        if c == '=' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_top_level_chains_only() {
        let segs = split_chain("cd /a && git status; echo \"x && y\" | wc -l");
        assert_eq!(
            segs,
            vec!["cd /a", "git status", "echo \"x && y\"", "wc -l"]
        );
    }

    #[test]
    fn normalize_dequotes_and_strips_rtk_and_env() {
        // transcript text vs rtk's stored original_cmd must hash equal
        assert_eq!(
            normalize("grep -rln 'session' src/ --include='*.rs'"),
            normalize("grep -rln session src/ --include=*.rs"),
        );
        assert_eq!(normalize("rtk git status"), normalize("git status"));
        assert_eq!(normalize("FOO=1 git  status"), normalize("git status"));
    }

    #[test]
    fn head_takes_basename() {
        assert_eq!(head("/usr/bin/git status"), "git");
        assert_eq!(head("git status"), "git");
    }

    #[test]
    fn split_chain_drops_empty_trailing_segments() {
        assert_eq!(split_chain("git status;"), vec!["git status"]);
        assert_eq!(split_chain(";;"), Vec::<String>::new());
    }

    #[test]
    fn split_chain_does_not_double_match_pipe_inside_or() {
        let segs = split_chain("git status || echo fail");
        assert_eq!(segs, vec!["git status", "echo fail"]);
    }

    #[test]
    fn split_chain_splits_on_newline() {
        let segs = split_chain("git status\ngit log");
        assert_eq!(segs, vec!["git status", "git log"]);
    }

    #[test]
    fn split_chain_never_splits_inside_single_quotes() {
        let segs = split_chain("echo 'a; b && c | d'");
        assert_eq!(segs, vec!["echo 'a; b && c | d'"]);
    }

    #[test]
    fn split_chain_unclosed_quote_does_not_panic() {
        let segs = split_chain("echo 'unterminated");
        assert_eq!(segs, vec!["echo 'unterminated"]);
    }

    #[test]
    fn tokenize_unclosed_quote_does_not_panic() {
        assert_eq!(normalize("echo 'unterminated"), "echo unterminated");
        assert_eq!(normalize("echo \"unterminated"), "echo unterminated");
    }

    #[test]
    fn env_assignment_mid_command_is_kept() {
        assert_eq!(normalize("git commit -m X=Y"), "git commit -m X=Y");
    }

    #[test]
    fn multiple_leading_env_assignments_are_stripped() {
        assert_eq!(normalize("FOO=1 BAR=2 rtk git status"), "git status");
        assert_eq!(normalize("rtk FOO=1 git status"), "git status");
    }

    #[test]
    fn head_of_empty_command_is_empty() {
        assert_eq!(head(""), "");
        assert_eq!(head("   "), "");
    }

    #[test]
    fn hash_matches_sha256_of_normalized_form() {
        assert_eq!(
            hash("rtk git status"),
            encoding::sha256_hex(&["git status"])
        );
    }

    #[test]
    fn chain_hashes_maps_each_segment() {
        let hashes = chain_hashes("git status && git log");
        assert_eq!(
            hashes,
            vec![
                encoding::sha256_hex(&["git status"]),
                encoding::sha256_hex(&["git log"]),
            ]
        );
    }

    #[test]
    fn head_on_raw_chain_text_is_garbage_by_contract() {
        // Regression guard: head/normalize/hash require a single segment
        // (see split_chain). Raw chain text must go through chain_hashes /
        // chain_head_hashes instead — these lock in today's degenerate
        // behavior so a future caller doesn't rediscover it the hard way.
        assert_eq!(head("FOO=1;ls"), "");
        assert_eq!(head("FOO=1 ; ls"), ";");
    }

    #[test]
    fn chain_head_hashes_maps_each_segment_head() {
        let hashes = chain_head_hashes("cd /x && git status");
        assert_eq!(hashes, vec![hash("cd"), hash("git")]);
    }

    #[test]
    fn chain_head_hashes_skips_env_only_segments() {
        let hashes = chain_head_hashes("A=1;ls");
        assert_eq!(hashes, vec![hash("ls")]);
    }

    #[test]
    fn chain_head_hashes_of_empty_command_is_empty() {
        assert_eq!(chain_head_hashes(""), Vec::<String>::new());
    }
}
