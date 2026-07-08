//! Pure, synchronous crontab-splice core (`String -> Result<String>`, no
//! process calls) plus the fail-closed process-facing layer built on top of
//! it: reading/writing the real `crontab` binary and the confirm -> re-read
//! -> backup -> write -> verify install/remove sequence.

use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::confirm;

/// Frozen exact-full-line markers. Matched only as a whole line, never as a
/// substring, and never changed across versions — anything that shipped with
/// an older marker text would otherwise become unremovable.
pub const MARK_START: &str = "# >>> runtab sync auto (managed — do not edit this block) >>>";
pub const MARK_END: &str = "# <<< runtab sync auto (managed) <<<";

const MINUTE_DIVISORS: [u32; 11] = [1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30];
const HOUR_DIVISORS: [u32; 8] = [1, 2, 3, 4, 6, 8, 12, 24];
const MINUTE_FLOOR: u32 = 15;

fn split_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.strip_suffix('\n').unwrap_or(content).split('\n').collect()
    }
}

fn join_lines(lines: &[&str]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

/// Single well-formed pass over `content`'s lines, splitting them into the
/// lines outside any managed block and the lines inside each one (in file
/// order), requiring strictly alternating start/end pairs. Any unpaired
/// start, unpaired end, or nested start aborts with `Err` and leaves
/// `content` untouched by the caller — this must never fall back to deleting
/// to EOF. Shared by `strip_managed` and `find_managed_entry` so both agree
/// on what "well-formed" means without scanning the file twice.
fn split_managed(content: &str) -> anyhow::Result<(Vec<&str>, Vec<Vec<&str>>)> {
    let mut outside: Vec<&str> = Vec::new();
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut current_block: Vec<&str> = Vec::new();
    let mut in_block = false;

    for line in split_lines(content) {
        if line == MARK_START {
            if in_block {
                anyhow::bail!("malformed managed block: nested start marker");
            }
            in_block = true;
            // Absorbs at most one blank separator line immediately preceding a
            // start marker, mirroring the single blank line `splice_install`
            // adds, so install/remove round-trips byte-identically.
            if outside.last().is_some_and(|last| last.is_empty()) {
                outside.pop();
            }
        } else if line == MARK_END {
            if !in_block {
                anyhow::bail!("malformed managed block: unpaired end marker with no matching start");
            }
            in_block = false;
            blocks.push(std::mem::take(&mut current_block));
        } else if in_block {
            current_block.push(line);
        } else {
            outside.push(line);
        }
    }

    if in_block {
        anyhow::bail!("malformed managed block: unpaired start marker with no matching end");
    }

    Ok((outside, blocks))
}

/// Removes every well-formed `MARK_START..MARK_END` block from `content`.
pub fn strip_managed(content: &str) -> anyhow::Result<String> {
    let (outside, _) = split_managed(content)?;
    Ok(join_lines(&outside))
}

/// Builds one managed block's body (markers + the single cron line). Does
/// not include a trailing newline; callers join it into the wider file.
pub fn managed_block(cron_expr: &str, command: &str) -> String {
    format!("{MARK_START}\n{cron_expr} {command}\n{MARK_END}")
}

/// Strips any existing managed block(s) from `content` (making install
/// idempotent) then appends `block`, separated from the rest of the file by
/// exactly one blank line when the file is non-empty.
pub fn splice_install(content: &str, block: &str) -> anyhow::Result<String> {
    let stripped = strip_managed(content)?;
    if stripped.is_empty() {
        Ok(format!("{block}\n"))
    } else {
        Ok(format!("{stripped}\n{block}\n"))
    }
}

/// Maps an `--interval` spec (`Nm` or `Nh`) to an explicit cron minute list,
/// phase-randomized by `offset` so installs don't all synchronize onto
/// `:00`/`:30`. Minutes must divide 60 and be at least the 15-minute floor;
/// hours must divide 24. Anything else — non-divisors, below-floor minutes,
/// unparseable specs — is a fail-closed `Err`.
pub fn interval_to_cron(spec: &str, offset: u32) -> anyhow::Result<String> {
    let spec = spec.trim();
    if let Some(digits) = spec.strip_suffix('m') {
        let n: u32 = digits
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid interval `{spec}`: expected e.g. `30m`"))?;
        if !MINUTE_DIVISORS.contains(&n) {
            anyhow::bail!("`{spec}` is not a divisor of 60 minutes; pick one of {MINUTE_DIVISORS:?}");
        }
        if n < MINUTE_FLOOR {
            anyhow::bail!(
                "`{spec}` is below the {MINUTE_FLOOR}-minute floor; use `runtab serve` for near-real-time sync"
            );
        }
        let list = (offset % n..60).step_by(n as usize).map(|m| m.to_string()).collect::<Vec<_>>().join(",");
        Ok(format!("{list} * * * *"))
    } else if let Some(digits) = spec.strip_suffix('h') {
        let n: u32 = digits
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid interval `{spec}`: expected e.g. `2h`"))?;
        if !HOUR_DIVISORS.contains(&n) {
            anyhow::bail!("`{spec}` is not a divisor of 24 hours; pick one of {HOUR_DIVISORS:?}");
        }
        let minute = offset % 60;
        Ok(format!("{minute} */{n} * * *"))
    } else {
        anyhow::bail!("invalid interval `{spec}`: expected `Nm` or `Nh` (e.g. `30m`, `2h`)");
    }
}

fn sanitize_path_for_cron(path: &str) -> anyhow::Result<String> {
    if path.chars().any(char::is_control) {
        anyhow::bail!("path contains a newline or control character: {path:?}");
    }
    if path.contains('\'') {
        anyhow::bail!("path contains a single quote, cannot safely embed in crontab: {path:?}");
    }
    if path.contains(MARK_START) || path.contains(MARK_END) {
        anyhow::bail!("path collides with the managed marker text: {path:?}");
    }
    Ok(path.replace('%', "\\%"))
}

/// Builds the full cron command field: env, binary, `--db`, the tick, and log
/// redirection. Paths are single-quoted and `%` is escaped as `\%` (cron
/// treats a bare `%` as end-of-command); any path containing a newline, CR,
/// control character, single quote, or marker text is rejected rather than
/// escaped. `data_dir` is `None` when the caller never had `XDG_DATA_HOME` set
/// interactively — cron's own `HOME`-based resolution then already lands on
/// the same data directory, so baking a redundant global-looking assignment
/// is skipped rather than papering over it.
pub fn build_command_line(bin: &str, db: &str, data_dir: Option<&str>, log: &str) -> anyhow::Result<String> {
    let bin = sanitize_path_for_cron(bin)?;
    let db = sanitize_path_for_cron(db)?;
    let log = sanitize_path_for_cron(log)?;
    let env_prefix = match data_dir {
        Some(dir) => format!("XDG_DATA_HOME='{}' ", sanitize_path_for_cron(dir)?),
        None => String::new(),
    };
    Ok(format!("{env_prefix}'{bin}' --db '{db}' sync run >> '{log}' 2>&1"))
}

/// Resolves the `crontab` binary to invoke: `RUNTAB_CRONTAB_CMD` (the
/// CLI-layer override tests and unusual `PATH`s use) if set, else plain
/// `crontab` off `PATH`.
pub fn crontab_cmd() -> String {
    std::env::var("RUNTAB_CRONTAB_CMD")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "crontab".to_string())
}

/// The cron expression and command line parsed out of a live managed block.
pub struct ManagedEntry {
    pub cron_expr: String,
    pub command: String,
}

/// Reads the live crontab and extracts the managed block's cron expression
/// and command, if one is installed. Reuses the same fail-closed read and
/// well-formed parse as install/remove, so a malformed block or an unreadable
/// crontab surfaces as `Err` rather than silently reporting "not installed".
pub fn find_managed_entry(cmd: &str) -> anyhow::Result<Option<ManagedEntry>> {
    let content = read_crontab(cmd)?;
    let (_, blocks) = split_managed(&content)?;
    for line in blocks.iter().flatten() {
        let mut fields = line.splitn(6, ' ');
        let cron_fields: Vec<&str> = fields.by_ref().take(5).collect();
        if cron_fields.len() == 5 {
            return Ok(Some(ManagedEntry {
                cron_expr: cron_fields.join(" "),
                command: fields.next().unwrap_or("").to_string(),
            }));
        }
    }
    Ok(None)
}

/// Whether a managed block is currently installed in the live crontab.
pub fn managed_block_present(cmd: &str) -> anyhow::Result<bool> {
    Ok(find_managed_entry(cmd)?.is_some())
}

/// Extracts the binary path baked into a managed command line, for the
/// binary-drift check in `sync auto status`. The command is always generated
/// by `build_command_line`, so a light quote-aware scan (rather than a full
/// shell parser) is enough: the binary is the first single-quoted token, or
/// the second one when an `XDG_DATA_HOME=` prefix was baked.
pub fn extract_bin(command: &str) -> Option<String> {
    let mut quoted = command.split('\'').skip(1).step_by(2);
    if command.trim_start().starts_with("XDG_DATA_HOME=") {
        quoted.next();
    }
    quoted.next().map(str::to_string)
}

/// Human-readable interval label (`30m`, `2h`, ...) for a cron expression
/// produced by `interval_to_cron`.
pub fn describe_interval(cron_expr: &str) -> String {
    let mut fields = cron_expr.split_whitespace();
    let minute_field = fields.next().unwrap_or("");
    let hour_field = fields.next().unwrap_or("");
    if let Some(n) = hour_field.strip_prefix("*/") {
        format!("{n}h")
    } else {
        format!("{}m", 60 / minute_field.split(',').count())
    }
}

/// Runs `<cmd> -l` under `LC_ALL=C` and classifies the result. Exit 0 is
/// success (empty stdout means a legitimately empty crontab). A non-zero
/// exit whose stderr contains "no crontab for" (case-insensitive) is treated
/// as an empty crontab. Anything else — spawn failure, `cron.deny`,
/// permission errors — is `Err`; misclassifying a real error as an empty
/// crontab would splice the managed block onto nothing and lose whatever was
/// actually there. There is deliberately no "assume empty" fallback.
pub fn read_crontab(cmd: &str) -> anyhow::Result<String> {
    let output = Command::new(cmd)
        .arg("-l")
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run `{cmd} -l`: {e}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if stderr.to_lowercase().contains("no crontab for") {
        return Ok(String::new());
    }
    anyhow::bail!(
        "`{cmd} -l` failed; refusing to assume an empty crontab.\nstderr: {}\n\
         run `crontab -e` once, then retry.",
        stderr.trim()
    )
}

/// Pipes `content` to `<cmd> -`, ensuring a trailing newline. `crontab -`
/// rejects invalid input atomically ("your crontab was not modified"), so a
/// non-zero exit here means nothing was written.
pub fn write_crontab(cmd: &str, content: &str) -> anyhow::Result<()> {
    let mut content = content.to_string();
    if !content.ends_with('\n') {
        content.push('\n');
    }
    let mut child = Command::new(cmd)
        .arg("-")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to run `{cmd} -`: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(content.as_bytes())
            .map_err(|e| anyhow::anyhow!("failed to write to `{cmd} -`: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| anyhow::anyhow!("failed to wait for `{cmd} -`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`{cmd} -` rejected the new crontab; your crontab was not modified: {}", stderr.trim());
    }
    Ok(())
}

/// Writes an unconditional timestamped backup of `content` (the fresh
/// snapshot taken right before a write) to `<data_dir>/crontab.bak.<ts>`,
/// disambiguating with a numeric suffix on the rare same-second collision,
/// then prunes to the last 5. Never a single fixed filename — re-running
/// would otherwise back up the mangled content over the only good copy.
fn write_backup(data_dir: &Path, content: &str) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(data_dir)?;
    let ts = crate::timeutil::now_epoch();
    let mut path = data_dir.join(format!("crontab.bak.{ts}"));
    let mut suffix = 1u32;
    while path.exists() {
        path = data_dir.join(format!("crontab.bak.{ts}.{suffix}"));
        suffix += 1;
    }
    std::fs::write(&path, content)
        .map_err(|e| anyhow::anyhow!("failed to write crontab backup to {}: {e}", path.display()))?;
    prune_backups(data_dir)?;
    Ok(path)
}

fn prune_backups(data_dir: &Path) -> anyhow::Result<()> {
    let mut backups: Vec<PathBuf> = std::fs::read_dir(data_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crontab.bak.")))
        .collect();
    backups.sort();
    for old in backups.drain(..backups.len().saturating_sub(5)) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// Re-reads the crontab and asserts every non-managed line is byte-identical
/// to `fresh_content` (the snapshot taken right before the write) and that
/// the managed block appears exactly once (`expect_block`) or not at all.
/// On any mismatch the caller's crontab may be wrong — always points at the
/// backup so the user can restore.
fn verify_write(cmd: &str, fresh_content: &str, backup_path: &Path, expect_block: bool) -> anyhow::Result<()> {
    let restore_hint = format!("restore with: crontab {}", backup_path.display());
    let expected_preserved = strip_managed(fresh_content)?;
    let reread_content = read_crontab(cmd)
        .map_err(|e| anyhow::anyhow!("read-back verification failed: {e}\n{restore_hint}"))?;
    let reread_preserved = strip_managed(&reread_content)
        .map_err(|e| anyhow::anyhow!("read-back verification failed: {e}\n{restore_hint}"))?;
    if reread_preserved != expected_preserved {
        anyhow::bail!(
            "read-back verification failed: existing crontab lines changed unexpectedly.\n{restore_hint}"
        );
    }
    let block_count = reread_content.matches(MARK_START).count();
    if expect_block && block_count != 1 {
        anyhow::bail!(
            "read-back verification failed: expected exactly one managed block, found {block_count}.\n{restore_hint}"
        );
    }
    if !expect_block && block_count != 0 {
        anyhow::bail!(
            "read-back verification failed: managed block still present after removal.\n{restore_hint}"
        );
    }
    Ok(())
}

/// Shared install/remove sequence: acquires `cron-edit.lock`, re-reads a
/// fresh snapshot, runs `transform` over it, and — if that actually changed
/// anything — backs up unconditionally, writes, and verifies the write.
/// Returns `false` without touching the crontab when `transform` is a no-op
/// (e.g. `remove` on a crontab with nothing managed installed), so the
/// caller can report "nothing to do" instead of a bogus "updated".
fn edit_crontab(
    cmd: &str,
    data_dir: &Path,
    expect_block: bool,
    transform: impl FnOnce(&str) -> anyhow::Result<String>,
) -> anyhow::Result<bool> {
    std::fs::create_dir_all(data_dir)?;
    let lock_path = data_dir.join("cron-edit.lock");
    let lock_file = File::create(&lock_path)
        .map_err(|e| anyhow::anyhow!("failed to open crontab edit lock {}: {e}", lock_path.display()))?;
    lock_file
        .lock()
        .map_err(|e| anyhow::anyhow!("failed to acquire crontab edit lock: {e}"))?;

    let fresh_content = read_crontab(cmd)?;
    let new_content = transform(&fresh_content)?;
    if new_content == fresh_content {
        return Ok(false);
    }

    let backup_path = write_backup(data_dir, &fresh_content)?;
    println!("backed up current crontab to {}", backup_path.display());

    write_crontab(cmd, &new_content)
        .map_err(|e| anyhow::anyhow!("{e}\nrestore with: crontab {}", backup_path.display()))?;

    verify_write(cmd, &fresh_content, &backup_path, expect_block)?;
    Ok(true)
}

pub struct InstallOpts<'a> {
    pub crontab_cmd: &'a str,
    pub data_dir: &'a Path,
    pub cron_expr: &'a str,
    pub command: &'a str,
    pub yes: bool,
}

/// Installs (or replaces, since `splice_install` first strips any existing
/// managed block) the managed crontab entry described by `opts`. Confirms
/// with the user first (unless `--yes`), then defers to `edit_crontab` for
/// the re-read/backup/write/verify sequence, so the confirmation prompt
/// can't widen the race between read and write.
pub fn install(opts: &InstallOpts) -> anyhow::Result<()> {
    let block = managed_block(opts.cron_expr, opts.command);

    let preview_content = read_crontab(opts.crontab_cmd)?;
    let preserved_lines = strip_managed(&preview_content)?.lines().count();

    println!("the following managed crontab block will be installed:\n");
    println!("{block}\n");
    println!("interval: {}", opts.cron_expr);
    println!("existing crontab lines preserved: {preserved_lines}");

    if opts.yes {
        println!("(--yes passed; skipping confirmation)");
    } else {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("refusing to edit crontab without confirmation on a non-interactive terminal; pass --yes");
        }
        if !confirm("install this crontab entry? [y/N] ", false)? {
            anyhow::bail!("aborted; crontab left unchanged");
        }
    }

    edit_crontab(opts.crontab_cmd, opts.data_dir, true, |fresh| splice_install(fresh, &block))?;
    println!("crontab updated: managed sync entry installed.");
    Ok(())
}

pub struct RemoveOpts<'a> {
    pub crontab_cmd: &'a str,
    pub data_dir: &'a Path,
}

/// Removes the managed crontab entry, prompt-free (the exact-match markers
/// are the safety boundary here, so cleanup should be frictionless). Same
/// re-read/backup/write/verify sequence as `install`, via `edit_crontab`.
pub fn remove(opts: &RemoveOpts) -> anyhow::Result<()> {
    if edit_crontab(opts.crontab_cmd, opts.data_dir, false, strip_managed)? {
        println!("crontab updated: managed sync entry removed.");
    } else {
        println!("no managed runtab crontab entry found; nothing to remove.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_managed_removes_a_single_block_and_preserves_other_bytes() {
        let block = managed_block("7,37 * * * *", "'/bin/runtab' sync run");
        let content = format!("SHELL=/bin/bash\n0 3 * * * /home/u/backup.sh\n\n{block}\n");
        let stripped = strip_managed(&content).unwrap();
        assert_eq!(stripped, "SHELL=/bin/bash\n0 3 * * * /home/u/backup.sh\n");
    }

    #[test]
    fn strip_managed_removes_multiple_blocks() {
        let block1 = managed_block("7,37 * * * *", "'/bin/runtab' sync run");
        let block2 = managed_block("0 */2 * * *", "'/bin/other' tick");
        let content = format!("a\n\n{block1}\nb\n\n{block2}\nc\n");
        let stripped = strip_managed(&content).unwrap();
        assert_eq!(stripped, "a\nb\nc\n");
    }

    #[test]
    fn strip_managed_on_unpaired_start_errs_without_deleting_to_eof() {
        let content = format!("keep me\n{MARK_START}\nsome line that never ends\n");
        assert!(strip_managed(&content).is_err());
    }

    #[test]
    fn strip_managed_on_unpaired_end_errs() {
        let content = format!("keep me\n{MARK_END}\nmore\n");
        assert!(strip_managed(&content).is_err());
    }

    #[test]
    fn strip_managed_on_nested_start_errs() {
        let content = format!("keep me\n{MARK_START}\ninner\n{MARK_START}\ninner2\n{MARK_END}\n{MARK_END}\n");
        assert!(strip_managed(&content).is_err());
    }

    #[test]
    fn install_remove_round_trips_byte_identical_from_nonempty_crontab() {
        let original = "SHELL=/bin/bash\n0 3 * * * /home/u/backup.sh\n";
        let block = managed_block("7,37 * * * *", "'/bin/runtab' sync run");

        let installed = splice_install(original, &block).unwrap();
        let removed = strip_managed(&installed).unwrap();
        assert_eq!(removed, original);

        let installed_again = splice_install(&removed, &block).unwrap();
        let removed_again = strip_managed(&installed_again).unwrap();
        assert_eq!(removed_again, original);
    }

    #[test]
    fn install_remove_round_trips_from_empty_crontab() {
        let original = "";
        let block = managed_block("7,37 * * * *", "'/bin/runtab' sync run");

        let installed = splice_install(original, &block).unwrap();
        let removed = strip_managed(&installed).unwrap();
        assert_eq!(removed, original);

        let installed_again = splice_install(&removed, &block).unwrap();
        let removed_again = strip_managed(&installed_again).unwrap();
        assert_eq!(removed_again, original);
    }

    #[test]
    fn interval_to_cron_30m_emits_a_phase_randomized_comma_list() {
        assert_eq!(interval_to_cron("30m", 7).unwrap(), "7,37 * * * *");
    }

    #[test]
    fn interval_to_cron_10m_divides_60_but_is_below_the_floor() {
        assert!(interval_to_cron("10m", 3).is_err());
    }

    #[test]
    fn interval_to_cron_45m_is_not_a_divisor_of_60() {
        assert!(interval_to_cron("45m", 0).is_err());
    }

    #[test]
    fn interval_to_cron_7m_is_not_a_divisor_of_60() {
        assert!(interval_to_cron("7m", 0).is_err());
    }

    #[test]
    fn interval_to_cron_2h_uses_the_offset_as_a_fixed_minute() {
        assert_eq!(interval_to_cron("2h", 5).unwrap(), "5 */2 * * *");
    }

    #[test]
    fn interval_to_cron_rejects_garbage() {
        assert!(interval_to_cron("thirty minutes", 0).is_err());
        assert!(interval_to_cron("", 0).is_err());
        assert!(interval_to_cron("-5m", 0).is_err());
    }

    #[test]
    fn build_command_line_quotes_paths_and_escapes_percent() {
        let line = build_command_line(
            "/opt/run tab/bin/runtab",
            "/home/u/My Files/100% done/runtab.db",
            Some("/home/u/.local/share"),
            "/home/u/.local/share/runtab/cron.log",
        )
        .unwrap();
        assert_eq!(
            line,
            "XDG_DATA_HOME='/home/u/.local/share' '/opt/run tab/bin/runtab' \
             --db '/home/u/My Files/100\\% done/runtab.db' \
             sync run >> '/home/u/.local/share/runtab/cron.log' 2>&1"
        );
    }

    #[test]
    fn build_command_line_omits_xdg_data_home_when_not_baked() {
        let line = build_command_line("/bin/runtab", "/db", None, "/log").unwrap();
        assert_eq!(line, "'/bin/runtab' --db '/db' sync run >> '/log' 2>&1");
    }

    #[test]
    fn build_command_line_rejects_dangerous_paths() {
        assert!(build_command_line("bin\nrm -rf /", "db", Some("data"), "log").is_err());
        assert!(build_command_line("bin", "it's/a/db", Some("data"), "log").is_err());
        assert!(build_command_line("bin", "db", Some("data"), MARK_START).is_err());
        assert!(build_command_line("bin", "db", Some("da\nta"), "log").is_err());
    }

    #[test]
    fn find_managed_entry_parses_the_installed_block() {
        let block = managed_block("7,37 * * * *", "'/bin/runtab' --db '/db' sync run >> '/log' 2>&1");
        let content = format!("SHELL=/bin/bash\n{block}\n");
        let stub = write_listing_stub(&content);
        let entry = find_managed_entry(stub.to_str().unwrap()).unwrap().expect("block present");
        assert_eq!(entry.cron_expr, "7,37 * * * *");
        assert_eq!(entry.command, "'/bin/runtab' --db '/db' sync run >> '/log' 2>&1");
        assert!(managed_block_present(stub.to_str().unwrap()).unwrap());
    }

    #[test]
    fn find_managed_entry_is_none_when_absent() {
        let stub = write_listing_stub("SHELL=/bin/bash\n0 3 * * * /home/u/backup.sh\n");
        assert!(find_managed_entry(stub.to_str().unwrap()).unwrap().is_none());
        assert!(!managed_block_present(stub.to_str().unwrap()).unwrap());
    }

    #[test]
    fn extract_bin_reads_the_baked_binary_with_and_without_env_prefix() {
        assert_eq!(
            extract_bin("XDG_DATA_HOME='/data' '/bin/runtab' --db '/db' sync run >> '/log' 2>&1"),
            Some("/bin/runtab".to_string())
        );
        assert_eq!(
            extract_bin("'/bin/runtab' --db '/db' sync run >> '/log' 2>&1"),
            Some("/bin/runtab".to_string())
        );
    }

    #[test]
    fn describe_interval_labels_minute_and_hour_forms() {
        assert_eq!(describe_interval("7,37 * * * *"), "30m");
        assert_eq!(describe_interval("5 */2 * * *"), "2h");
    }

    /// Writes a tiny stub `crontab` that only serves `-l` with fixed content,
    /// for tests exercising the read-side parsing helpers in isolation.
    fn write_listing_stub(content: &str) -> PathBuf {
        use std::io::Write as _;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("runtab_cron_listing_{}_{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("current");
        std::fs::write(&fixture, content).unwrap();
        let script = dir.join("crontab-stub.sh");
        let mut file = File::create(&script).unwrap();
        writeln!(file, "#!/bin/sh\ncat '{}'", fixture.display()).unwrap();
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        script
    }
}
