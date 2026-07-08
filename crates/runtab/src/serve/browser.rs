use std::process::{Command, Stdio};

/// Best-effort open of the dashboard in the user's browser. Silently does
/// nothing on failure — the URL is always printed by the caller as the fallback.
pub fn open(url: &str) {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else if cfg!(target_os = "windows") {
        &[("cmd", &["/C", "start", ""])]
    } else {
        &[("xdg-open", &[]), ("gio", &["open"])]
    };

    for (bin, args) in candidates {
        let mut cmd = Command::new(bin);
        cmd.args(*args)
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if cmd.spawn().is_ok() {
            return;
        }
    }
}
