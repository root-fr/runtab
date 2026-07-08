use std::path::PathBuf;

/// Sync server base URL: `RUNTAB_SERVER_URL` if set, else the hosted service.
pub fn server_url() -> String {
    std::env::var("RUNTAB_SERVER_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://api.runtab.ai".to_string())
}

/// The runtab data directory (`$XDG_DATA_HOME/runtab` or `~/.local/share/runtab`),
/// used for the token file fallback when no OS keyring is available.
pub fn data_dir() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| crate::home_dir().map(|h| h.join(".local").join("share")))
        .ok_or_else(|| anyhow::anyhow!("cannot determine data directory (set XDG_DATA_HOME or HOME)"))?;
    Ok(base.join("runtab"))
}
