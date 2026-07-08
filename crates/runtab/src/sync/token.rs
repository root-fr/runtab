use std::fs;
use std::path::PathBuf;

use super::config::data_dir;

const SERVICE: &str = "runtab-sync";

/// Bearer-token store: OS keyring first, private-file fallback second. The
/// fallback file is created `0600` and its use is warned once so the user knows
/// the token is on disk rather than in the system secret store.
pub struct TokenStore;

impl TokenStore {
    pub fn store(account: &str, token: &str) -> anyhow::Result<()> {
        if keyring_set(account, token).is_ok() {
            return Ok(());
        }
        warn_fallback();
        write_file(token)
    }

    pub fn load(account: &str) -> anyhow::Result<Option<String>> {
        if let Ok(Some(t)) = keyring_get(account) {
            return Ok(Some(t));
        }
        read_file()
    }

    pub fn delete(account: &str) -> anyhow::Result<()> {
        let _ = keyring_delete(account);
        let path = token_path()?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// True if the 0600 file fallback currently holds a token, independent of
    /// what the keyring has.
    pub fn file_has_token() -> bool {
        matches!(read_file(), Ok(Some(_)))
    }

    /// Mirrors the keyring-held token for `account` into the 0600 file
    /// fallback so a cron tick can read it. `Ok(false)` means the keyring had
    /// nothing to mirror — the caller decides whether that's fine (there was
    /// never a token) or an error (the user just consented expecting it to
    /// work); `Err` is a hard failure writing the file.
    pub fn mirror_to_file(account: &str) -> anyhow::Result<bool> {
        match keyring_get(account) {
            Ok(Some(token)) => {
                write_file(&token)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

fn keyring_set(account: &str, token: &str) -> Result<(), keyring::Error> {
    keyring::Entry::new(SERVICE, account)?.set_password(token)
}

fn keyring_get(account: &str) -> Result<Option<String>, keyring::Error> {
    match keyring::Entry::new(SERVICE, account)?.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}

fn keyring_delete(account: &str) -> Result<(), keyring::Error> {
    keyring::Entry::new(SERVICE, account)?.delete_credential()
}

fn token_path() -> anyhow::Result<PathBuf> {
    Ok(data_dir()?.join("sync_token"))
}

fn write_file(token: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let path = token_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Create with 0600 up front so there is no window where the bearer token is
    // world-readable under the default umask (a chmod-after-write TOCTOU leak).
    let mut file = private_create(&path)?;
    file.write_all(token.as_bytes())?;
    Ok(())
}

#[cfg(unix)]
fn private_create(path: &std::path::Path) -> anyhow::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?)
}

#[cfg(not(unix))]
fn private_create(path: &std::path::Path) -> anyhow::Result<fs::File> {
    Ok(fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?)
}

fn read_file() -> anyhow::Result<Option<String>> {
    let path = token_path()?;
    match fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        _ => Ok(None),
    }
}

pub(crate) fn warn_fallback() {
    eprintln!(
        "runtab: no OS keyring available; storing the sync token in a 0600 file \
         under the data directory instead."
    );
}
