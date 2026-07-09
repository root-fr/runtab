pub mod cron;
pub mod client;
pub mod config;
mod login;
mod pull;
mod push;
mod review;
mod run;
pub mod token;

pub use client::{SyncClient, SyncError};
pub use pull::{pull_all, PullOutcome};
pub use push::{push_all, PushOutcome};

use std::io::{IsTerminal, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::ledger::Ledger;
use crate::pricing::Pricing;
use token::TokenStore;

type SyncRoundTrip = anyhow::Result<(Ledger, Result<(PushOutcome, PullOutcome), SyncError>)>;

pub enum Cmd {
    Login,
    Status,
    Now,
    Off,
    Delete,
    Run { verbose: bool },
    AutoOn { interval: Option<String>, yes: bool },
    AutoOff,
    AutoStatus,
}

pub fn dispatch(ledger: Ledger, pricing: &Pricing, db_path: &Path, cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Status => status(&ledger),
        Cmd::Off => off(&ledger),
        Cmd::Login => login(ledger),
        Cmd::Now => now(ledger),
        Cmd::Delete => delete(ledger),
        Cmd::Run { verbose } => run::run_once(ledger, pricing, verbose),
        Cmd::AutoOn { interval, yes } => auto_on(&ledger, db_path, interval, yes),
        Cmd::AutoOff => auto_off(),
        Cmd::AutoStatus => auto_status(),
    }
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

fn login(ledger: Ledger) -> anyhow::Result<()> {
    let server = config::server_url();
    let machine_name = ledger.machine_name().to_string();
    let client = SyncClient::new(&server)?;
    let m = Mutex::new(ledger);
    runtime()?.block_on(login::run(&m, &client, &machine_name, &server))?;
    let ledger = m.into_inner().map_err(|_| anyhow::anyhow!("ledger lock poisoned"))?;
    // The pre-sync review is the consent moment (spec guardrail). Run it unless it
    // was already completed elsewhere (e.g. the dashboard review screen), so those
    // exclusions are honoured rather than silently overwritten.
    if !ledger.projects_reviewed()? {
        review::run(&ledger)?;
    }
    Ok(())
}

fn status(ledger: &Ledger) -> anyhow::Result<()> {
    let s = ledger.sync_state()?;
    let pending = ledger.pending_push_count()?;
    let state = if !s.enabled {
        "off"
    } else if s.degraded {
        "degraded"
    } else {
        "ok"
    };
    println!("sync: {state}");
    if let Some(email) = &s.account_email {
        println!("  account:      {email}");
    }
    println!("  server_seq:   {}", s.pull_cursor);
    println!("  pending push: {pending}");
    if let Some(msg) = &s.message {
        println!("  message:      {msg}");
    }
    println!("  auto-sync:    {}", auto_sync_summary());
    println!("  machines:");
    for machine in ledger.machine_stats()? {
        let mark = if machine.is_current { "*" } else { " " };
        println!(
            "   {mark} {} ({}) — {} events",
            machine.machine_name, machine.machine_id, machine.event_count
        );
    }
    Ok(())
}

fn now(ledger: Ledger) -> anyhow::Result<()> {
    let s = ledger.sync_state()?;
    if !s.enabled {
        anyhow::bail!("sync is off; run `runtab sync login` first");
    }
    if !ledger.projects_reviewed()? {
        anyhow::bail!(
            "projects not yet reviewed; run `runtab sync login` again or open the dashboard to \
             choose which projects sync before the first push"
        );
    }
    let email = s
        .account_email
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no account on record; run `runtab sync login`"))?;
    let token = TokenStore::load(&email)?
        .ok_or_else(|| anyhow::anyhow!("no sync token found; run `runtab sync login`"))?;
    let (led, result) = push_pull_once(ledger, s.server_url.as_deref(), &token)?;
    apply_sync_outcome(&led, result)
}

/// Shared by `now()` and `run::run_once`: resolves the server URL, builds a
/// client, and runs one push+pull round trip over the ledger (handed through
/// a `Mutex` for the async tasks and handed back once they're done). Each
/// caller keeps its own precondition policy and applies the result with
/// `apply_sync_outcome`.
fn push_pull_once(ledger: Ledger, server_url: Option<&str>, token: &str) -> SyncRoundTrip {
    let server = server_url.map(str::to_string).unwrap_or_else(config::server_url);
    let machine_id = ledger.machine_id().to_string();
    let client = SyncClient::new(&server)?;
    let m = Mutex::new(ledger);

    let result = runtime()?.block_on(async {
        let pushed = push_all(&m, &client, token).await?;
        let pulled = pull_all(&m, &client, token, &machine_id).await?;
        Ok::<_, SyncError>((pushed, pulled))
    });
    let led = m.into_inner().map_err(|_| anyhow::anyhow!("ledger lock poisoned"))?;
    Ok((led, result))
}

/// Shared by `now()` and `run::run_once`: applies a push+pull result to the
/// ledger's degraded state and reports it. Quota-reached is informational
/// (not degraded) so a cron-only user still sees it in `sync status` instead
/// of it being erased by the next tick.
fn apply_sync_outcome(
    led: &Ledger,
    result: Result<(PushOutcome, PullOutcome), SyncError>,
) -> anyhow::Result<()> {
    match result {
        Ok((pushed, pulled)) => {
            if pushed.quota_reached {
                led.set_degraded(false, Some("daily quota reached; remaining events sync on the next run"))?;
                println!(
                    "sync: pushed {} events, pulled {} events — daily quota reached, remaining events \
                     will sync automatically on the next run (or tomorrow); `runtab serve` keeps this flowing \
                     in the background",
                    pushed.pushed, pulled.pulled
                );
            } else {
                led.set_degraded(false, None)?;
                println!(
                    "sync complete: pushed {} events, pulled {} events",
                    pushed.pushed, pulled.pulled
                );
            }
            Ok(())
        }
        Err(SyncError::Unauthorized) => {
            led.set_degraded(true, Some("sync revoked — run `runtab sync login` or `runtab sync off`"))?;
            anyhow::bail!("sync token was rejected (401); run `runtab sync login` again")
        }
        Err(e) => {
            led.set_degraded(true, Some(&e.to_string()))?;
            anyhow::bail!("sync degraded: {e}")
        }
    }
}

fn off(ledger: &Ledger) -> anyhow::Result<()> {
    let email = ledger.sync_state()?.account_email;
    ledger.disable_sync()?;
    if let Some(email) = email {
        let _ = TokenStore::delete(&email);
    }
    println!("sync is off; local data is unchanged.");
    maybe_remove_managed_cron()?;
    Ok(())
}

fn delete(ledger: Ledger) -> anyhow::Result<()> {
    let s = ledger.sync_state()?;
    let email = s
        .account_email
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no account on record; nothing to delete"))?;
    let token = TokenStore::load(&email)?
        .ok_or_else(|| anyhow::anyhow!("no sync token found; run `runtab sync login`"))?;
    let confirm = prompt("This wipes your synced account on the server. Type 'yes' to confirm: ")?;
    if confirm != "yes" {
        anyhow::bail!("aborted");
    }
    let server = s.server_url.clone().unwrap_or_else(config::server_url);
    let client = SyncClient::new(&server)?;
    let res = runtime()?.block_on(client.delete_account(&token));
    let _ = TokenStore::delete(&email);
    ledger.reset_sync()?;
    match res {
        Ok(r) => println!(
            "account deleted: {} events and {} machines removed on the server.",
            r.events_removed, r.machines_removed
        ),
        Err(SyncError::Unauthorized) => {
            println!("server already had no such account; local sync state cleared.")
        }
        Err(e) => anyhow::bail!("server delete failed: {e}"),
    }
    maybe_remove_managed_cron()?;
    Ok(())
}

fn prompt(label: &str) -> anyhow::Result<String> {
    print!("{label}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

pub(crate) fn confirm(label: &str, default_yes: bool) -> anyhow::Result<bool> {
    let answer = prompt(label)?.to_lowercase();
    Ok(answer == "y" || answer == "yes" || (default_yes && answer.is_empty()))
}

/// After `off`/`delete` clear sync, offers to also remove a managed auto-sync
/// crontab entry if one is present (default yes — leaving a cron job that
/// scan-onlys forever is harmless but surprising). Non-interactive contexts
/// and root/sudo just get a notice, never a hard failure of `off`/`delete`.
fn maybe_remove_managed_cron() -> anyhow::Result<()> {
    let cmd = cron::crontab_cmd();
    let present = match cron::managed_block_present(&cmd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("runtab: could not check for a managed auto-sync crontab entry: {e}");
            return Ok(());
        }
    };
    if !present {
        return Ok(());
    }
    if guard_not_root().is_err() {
        eprintln!(
            "runtab: an auto-sync crontab entry is still installed, but it can't be removed while \
             running as root/sudo; run `runtab sync auto off` as the login user."
        );
        return Ok(());
    }
    let remove = if !std::io::stdin().is_terminal() {
        eprintln!("runtab: an auto-sync crontab entry is still installed; run `runtab sync auto off` to remove it.");
        false
    } else {
        confirm("auto-sync cron entry found — remove it too? [Y/n] ", true)?
    };
    if remove {
        let data_dir = config::data_dir()?;
        cron::remove(&cron::RemoveOpts { crontab_cmd: &cmd, data_dir: &data_dir })?;
    }
    Ok(())
}

/// True if a `runtab serve` dashboard looks reachable on its usual port — a
/// best-effort TCP probe, not a dedicated endpoint (the dashboard itself is
/// out of scope for this feature).
fn serve_probably_running() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], crate::serve::resolve_port(None)));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

/// The `auto-sync:` line shown by `sync status`.
fn auto_sync_summary() -> String {
    if serve_probably_running() {
        return "serve running".to_string();
    }
    match cron::find_managed_entry(&cron::crontab_cmd()) {
        Ok(Some(entry)) => format!("cron every {}", cron::describe_interval(&entry.cron_expr)),
        _ => "none".to_string(),
    }
}

#[cfg(unix)]
fn current_euid() -> Option<u32> {
    let output = std::process::Command::new("id").arg("-u").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Refuses to touch the crontab while running as root or under `sudo` — a
/// common reflex after a permission error is to retry with `sudo`, which then
/// installs into root's crontab; an unprivileged `sync auto off` later can't
/// find it there. Fail-closed: an euid that can't be determined is treated
/// the same as root rather than let root through by accident.
#[cfg(unix)]
fn guard_not_root() -> anyhow::Result<()> {
    if std::env::var_os("SUDO_USER").is_some() {
        anyhow::bail!(
            "refusing to edit a crontab while running under sudo (SUDO_USER is set); run \
             `runtab sync auto` as the login user, not root"
        );
    }
    match current_euid() {
        Some(0) => anyhow::bail!("refusing to edit root's crontab; run `runtab sync auto` as the login user"),
        Some(_) => Ok(()),
        None => anyhow::bail!("could not determine the current user; run `runtab sync auto` as your login user"),
    }
}

#[cfg(not(unix))]
fn guard_not_root() -> anyhow::Result<()> {
    anyhow::bail!("`runtab sync auto` needs `crontab`, which is only available on Linux/macOS; use `runtab serve` instead")
}

fn current_exe_canonical() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.canonicalize().ok())
}

fn tail_last_line(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()?.lines().last().map(str::to_string)
}

/// If the sync token is only in the OS keyring, cron (no session bus) cannot
/// read it. Offers (explicit consent, `--yes` or an interactive prompt) to
/// mirror it into the 0600 file fallback; refuses to install without either
/// consent or an existing file token, and refuses just as hard if the
/// keyring turns out to have nothing to mirror after all — `sync auto on`
/// must never install a cron tick that can't authenticate.
fn ensure_cron_readable_token(account_email: Option<&str>, yes: bool) -> anyhow::Result<()> {
    let Some(email) = account_email else {
        return Ok(());
    };
    if TokenStore::file_has_token() {
        return Ok(());
    }
    token::warn_fallback();
    let consent = if yes {
        true
    } else if !std::io::stdin().is_terminal() {
        false
    } else {
        confirm(
            "cron has no session bus to read the OS keyring — mirror the sync token into that \
             0600 file so `sync run` can read it? [y/N] ",
            false,
        )?
    };
    if !consent {
        anyhow::bail!(
            "cron cannot read a keyring-only sync token; rerun with --yes to mirror it into the \
             0600 file fallback, or use `runtab serve` instead (it stays attached to your session)"
        );
    }
    if !TokenStore::mirror_to_file(email)? {
        anyhow::bail!("could not read the sync token to mirror it");
    }
    Ok(())
}

/// The `XDG_DATA_HOME` value to bake into the cron line, or `None` to leave
/// it unset (matching the caller's own interactive environment). This must
/// be the raw env value, not `config::data_dir()` — the latter already has
/// `/runtab` appended, so baking it would double that suffix under cron.
fn baked_xdg_data_home(raw: Option<&std::ffi::OsStr>) -> Option<String> {
    raw.filter(|v| !v.is_empty()).map(|v| v.to_string_lossy().into_owned())
}

fn auto_on(ledger: &Ledger, db_path: &Path, interval: Option<String>, yes: bool) -> anyhow::Result<()> {
    guard_not_root()?;

    let bin = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("cannot resolve the runtab binary path: {e}"))?;
    let bin = bin
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("cannot canonicalize {}: {e}", bin.display()))?;
    let db = db_path.canonicalize().unwrap_or_else(|_| db_path.to_path_buf());
    let data_dir = config::data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let log = data_dir.join("cron.log");

    let baked_data_dir = baked_xdg_data_home(std::env::var_os("XDG_DATA_HOME").as_deref());

    let command = cron::build_command_line(
        &bin.to_string_lossy(),
        &db.to_string_lossy(),
        baked_data_dir.as_deref(),
        &log.to_string_lossy(),
    )?;

    let spec = interval.unwrap_or_else(|| "30m".to_string());
    let offset = crate::encoding::random_u32(60);
    let cron_expr = cron::interval_to_cron(&spec, offset)?;

    let s = ledger.sync_state()?;
    ensure_cron_readable_token(s.account_email.as_deref(), yes)?;

    if !s.enabled || s.account_email.is_none() {
        eprintln!("runtab: not logged in yet — `sync run` will scan only until `runtab sync login` completes.");
    } else if !ledger.projects_reviewed()? {
        eprintln!("runtab: projects not yet reviewed — `sync run` will scan only until review completes.");
    }
    if serve_probably_running() {
        eprintln!(
            "runtab: `runtab serve` looks like it's already running on this machine and syncs every \
             30s; auto-sync is harmless alongside it but redundant."
        );
    }

    cron::install(&cron::InstallOpts {
        crontab_cmd: &cron::crontab_cmd(),
        data_dir: &data_dir,
        cron_expr: &cron_expr,
        command: &command,
        yes,
    })
}

fn auto_off() -> anyhow::Result<()> {
    guard_not_root()?;
    let data_dir = config::data_dir()?;
    cron::remove(&cron::RemoveOpts { crontab_cmd: &cron::crontab_cmd(), data_dir: &data_dir })
}

fn auto_status() -> anyhow::Result<()> {
    let cmd = cron::crontab_cmd();
    match cron::find_managed_entry(&cmd) {
        Ok(Some(entry)) => {
            println!("auto-sync: installed");
            println!("  interval: {} ({})", cron::describe_interval(&entry.cron_expr), entry.cron_expr);
            println!("  line:     {} {}", entry.cron_expr, entry.command);
            if let (Some(baked), Some(current)) = (cron::extract_bin(&entry.command), current_exe_canonical()) {
                if baked != current.to_string_lossy() {
                    println!(
                        "  warning:  baked binary path `{baked}` no longer matches the current binary \
                         (`{}`) — the executable moved; reinstall with `runtab sync auto on`",
                        current.display()
                    );
                }
            }
            match tail_last_line(&config::data_dir()?.join("cron.log")) {
                Some(line) => println!("  last run: {line}"),
                None => println!("  last run: (no cron.log entries yet)"),
            }
        }
        Ok(None) => println!("auto-sync: not installed"),
        Err(e) => println!("auto-sync: could not read the crontab: {e}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_xdg_data_home_uses_the_raw_env_value_not_the_runtab_subdir() {
        let raw = std::ffi::OsStr::new("/home/u/.local/share");
        let baked = baked_xdg_data_home(Some(raw)).unwrap();
        assert_eq!(baked, "/home/u/.local/share");
        assert!(!baked.ends_with("/runtab"), "must not bake config::data_dir()'s /runtab suffix");
    }

    #[test]
    fn baked_xdg_data_home_is_none_when_unset_or_empty() {
        assert_eq!(baked_xdg_data_home(None), None);
        assert_eq!(baked_xdg_data_home(Some(std::ffi::OsStr::new(""))), None);
    }
}
