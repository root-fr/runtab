use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use runtab::sync::cron::{install, managed_block, remove, splice_install, InstallOpts, RemoveOpts, MARK_START};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "runtab_cron_install_{label}_{}_{nanos}_{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a stub `crontab` to `dir/crontab-stub.sh` and points
/// `RUNTAB_CRONTAB_CMD` at it (the CLI-layer override the real command uses;
/// tests pass the same path directly into the opts structs so this never
/// touches a real crontab). `-l` echoes `dir/current` (or fails, if
/// `dir/fail_code` exists, with the recorded message/exit code); `-` records
/// stdin to `dir/stdin_recorded` and mirrors it into `dir/current` so a
/// following `-l` reflects the write, matching real crontab semantics.
fn write_stub(dir: &Path) -> PathBuf {
    let script = dir.join("crontab-stub.sh");
    let body = r#"#!/bin/sh
set -e
DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
case "$1" in
  -l)
    if [ -f "$DIR/fail_code" ]; then
      cat "$DIR/fail_msg" >&2
      exit "$(cat "$DIR/fail_code")"
    fi
    cat "$DIR/current" 2>/dev/null || true
    ;;
  -)
    cat > "$DIR/stdin_recorded"
    cp "$DIR/stdin_recorded" "$DIR/current"
    ;;
  *)
    echo "stub crontab: unsupported args: $*" >&2
    exit 1
    ;;
esac
"#;
    std::fs::write(&script, body).unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
    std::env::set_var("RUNTAB_CRONTAB_CMD", &script);
    script
}

fn set_current(dir: &Path, content: &str) {
    std::fs::write(dir.join("current"), content).unwrap();
}

fn set_failing(dir: &Path, code: &str, msg: &str) {
    std::fs::write(dir.join("fail_code"), code).unwrap();
    std::fs::write(dir.join("fail_msg"), msg).unwrap();
}

#[test]
fn cron_install_then_remove_round_trips_to_the_exact_fixture() {
    let stub_dir = temp_dir("stub");
    let data_dir = temp_dir("data");
    let stub = write_stub(&stub_dir);

    let fixture = "SHELL=/bin/bash\n0 3 * * * /home/u/unrelated-job.sh\n";
    set_current(&stub_dir, fixture);

    let cmd = stub.to_str().unwrap();
    let opts = InstallOpts {
        crontab_cmd: cmd,
        data_dir: &data_dir,
        cron_expr: "7,37 * * * *",
        command: "'/bin/runtab' sync run",
        yes: true,
    };
    install(&opts).expect("install should succeed against the stub");

    let recorded = std::fs::read_to_string(stub_dir.join("stdin_recorded")).unwrap();
    let block = managed_block("7,37 * * * *", "'/bin/runtab' sync run");
    let expected = splice_install(fixture, &block).unwrap();
    assert_eq!(recorded, expected, "installed crontab must be the unrelated job plus one managed block");
    assert!(recorded.contains("/home/u/unrelated-job.sh"));
    assert_eq!(recorded.matches(MARK_START).count(), 1);

    let backups: Vec<_> = std::fs::read_dir(&data_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("crontab.bak.")))
        .collect();
    assert_eq!(backups.len(), 1, "install must write exactly one backup of the pre-install crontab");
    let backup_content = std::fs::read_to_string(backups[0].path()).unwrap();
    assert_eq!(backup_content, fixture, "the backup must hold the crontab as it was before install");

    let remove_opts = RemoveOpts { crontab_cmd: cmd, data_dir: &data_dir };
    remove(&remove_opts).expect("remove should succeed against the stub");

    let recorded_after_remove = std::fs::read_to_string(stub_dir.join("stdin_recorded")).unwrap();
    assert_eq!(recorded_after_remove, fixture, "remove must return the crontab to the exact original fixture");
}

#[test]
fn cron_install_permission_denied_on_list_aborts_before_any_write() {
    let stub_dir = temp_dir("stub_fail");
    let data_dir = temp_dir("data_fail");
    let stub = write_stub(&stub_dir);
    set_failing(&stub_dir, "2", "crontab: permission denied");

    let cmd = stub.to_str().unwrap();
    let opts = InstallOpts {
        crontab_cmd: cmd,
        data_dir: &data_dir,
        cron_expr: "7,37 * * * *",
        command: "'/bin/runtab' sync run",
        yes: true,
    };
    let err = install(&opts).expect_err("install must abort when `-l` reports permission denied");
    assert!(err.to_string().to_lowercase().contains("permission denied"));
    assert!(!stub_dir.join("stdin_recorded").exists(), "`crontab -` must never be invoked");

    let remove_opts = RemoveOpts { crontab_cmd: cmd, data_dir: &data_dir };
    let err = remove(&remove_opts).expect_err("remove must abort when `-l` reports permission denied too");
    assert!(err.to_string().to_lowercase().contains("permission denied"));
    assert!(!stub_dir.join("stdin_recorded").exists());
}
