//! `status` writes nothing in the registry (handoff 013d-4, item 2; REQ-SCHED-006, REQ-OBS-009;
//! external design §19.3) and `MIMLYS_RUN_DIR` is a debug-build seam (item 3; REQ-SCHED-006), through
//! the real binary. The verdicts are states of the file system: what exists afterwards, a file's
//! bytes, and (Unix) the directory's mode and its modification time, which the old probe file and
//! the old `chmod` changed even though the probe was removed again.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n",
                dir.path().join("data").to_string_lossy()
            ),
        )
        .unwrap();
        Self { dir, config }
    }

    /// A directory under the test's own, for what the test arranges.
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `status` as the user's runtime would run it against `registry`, in the given form.
    fn status(&self, registry: &Path, json: bool) -> (String, Value) {
        let mut cmd = common::mimlys();
        cmd.env("MIMLYS_RUN_DIR", registry)
            .env("MIMLYS_CONFIG", "")
            .arg("--config")
            .arg(&self.config);
        if json {
            cmd.arg("--json");
        }
        cmd.arg("status");
        let out = run_bounded(&mut cmd, self.dir.path());
        assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
        let value = if json {
            serde_json::from_str(out.stdout.trim()).unwrap()
        } else {
            Value::Null
        };
        (out.stdout, value)
    }
}

/// The names in a directory, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Sets the modification time of a directory to an hour ago (Unix), and returns it: any entry made
/// or removed in it afterwards moves it to now.
#[cfg(unix)]
fn age(dir: &Path) -> std::time::SystemTime {
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::open(dir).unwrap().set_modified(old).unwrap();
    std::fs::metadata(dir).unwrap().modified().unwrap()
}

/// REQ-SCHED-006, REQ-OBS-009 (013d-4): against a registry whose directory is not there yet,
/// `status` says `not yet used (no runtime of yours has taken a slot)`, counts it as ready, and
/// makes nothing: not the directory, not a probe file, not its parent's contents.
#[test]
fn req_obs_009_status_against_a_missing_registry_creates_nothing() {
    let env = Env::new();
    let parent = env.path("parent");
    std::fs::create_dir(&parent).unwrap();
    let registry = parent.join("run");
    #[cfg(unix)]
    let before = age(&parent);
    let (text, _) = env.status(&registry, false);
    assert!(
        text.contains("admission: not yet used (no runtime of yours has taken a slot)"),
        "{text}"
    );
    let (_, value) = env.status(&registry, true);
    assert_eq!(
        value["machine"]["admission"],
        serde_json::json!({"state": "ready", "not_yet_used": true})
    );
    assert_eq!(value["machine"]["slots"]["taken"], 0);
    assert!(!registry.exists(), "status made the registry's directory");
    assert!(names(&parent).is_empty(), "{:?}", names(&parent));
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&parent).unwrap().modified().unwrap(),
        before,
        "the parent directory was written to"
    );
}

/// REQ-SCHED-006, REQ-OBS-009 (013d-4): against a registry that is there and usable, `status` says
/// `ready` and leaves the directory as it was: no probe file (not even one made and removed, which
/// moves the directory's time) and no change of its mode (the first take makes it private; a read
/// does not). Unix.
#[cfg(unix)]
#[test]
fn req_obs_009_status_against_a_present_registry_changes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let registry = env.path("run");
    std::fs::create_dir(&registry).unwrap();
    std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o755)).unwrap();
    let before = age(&registry);
    let (text, _) = env.status(&registry, false);
    assert!(
        text.contains("admission: ready (0 of 2 slots taken)"),
        "{text}"
    );
    let (_, value) = env.status(&registry, true);
    assert_eq!(
        value["machine"]["admission"],
        serde_json::json!({"state": "ready"})
    );
    let meta = std::fs::metadata(&registry).unwrap();
    assert!(names(&registry).is_empty(), "{:?}", names(&registry));
    assert_eq!(meta.modified().unwrap(), before, "a file was made in it");
    assert_eq!(meta.permissions().mode() & 0o777, 0o755, "the mode changed");
}

/// REQ-SCHED-006, REQ-OBS-009 (013d-4): against a registry that cannot be used, `status` says
/// `unavailable (<reason>)` and changes no file: a file in the directory's place keeps its bytes
/// and its parent keeps its contents. Every platform. On Unix, a link in its place is unusable too
/// and what it points at is left alone.
#[test]
fn req_obs_009_status_against_an_unusable_registry_changes_no_file() {
    let env = Env::new();
    let parent = env.path("parent");
    std::fs::create_dir(&parent).unwrap();
    let file = parent.join("run");
    std::fs::write(&file, b"not a directory").unwrap();
    #[cfg(unix)]
    let before = age(&parent);
    let (text, _) = env.status(&file, false);
    assert!(
        text.contains("admission: unavailable (") && text.contains("is not a directory of its own"),
        "{text}"
    );
    let (_, value) = env.status(&file, true);
    assert_eq!(value["machine"]["admission"]["state"], "unavailable");
    assert_eq!(std::fs::read(&file).unwrap(), b"not a directory");
    assert_eq!(names(&parent), ["run"]);
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&parent).unwrap().modified().unwrap(),
        before,
        "the parent directory was written to"
    );

    #[cfg(unix)]
    {
        let target = env.path("target");
        std::fs::create_dir(&target).unwrap();
        let target_before = age(&target);
        let link = env.path("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (text, _) = env.status(&link, false);
        assert!(
            text.contains("admission: unavailable (")
                && text.contains("is not a directory of its own"),
            "{text}"
        );
        assert!(names(&target).is_empty(), "{:?}", names(&target));
        assert_eq!(
            std::fs::metadata(&target).unwrap().modified().unwrap(),
            target_before
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

/// REQ-SCHED-006 (013d-4): `MIMLYS_RUN_DIR` names the registry in a debug build, which the tests
/// use, and is ignored by a release build (`cargo test --release --test registry_read_only`
/// runs this with the other expectation): the registry is then the platform's, and nothing is
/// made where the variable pointed.
#[test]
fn req_sched_006_run_dir_is_read_by_a_debug_build_only() {
    let env = Env::new();
    let named = env.path("named-by-the-variable");
    let xdg = env.path("xdg");
    std::fs::create_dir(&xdg).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.env("MIMLYS_RUN_DIR", &named)
        .env("XDG_RUNTIME_DIR", &xdg)
        .env("MIMLYS_CONFIG", "")
        .arg("--config")
        .arg(&env.config)
        .args(["--json", "status"]);
    let out = run_bounded(&mut cmd, env.dir.path());
    assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
    let value: Value = serde_json::from_str(out.stdout.trim()).unwrap();
    let shown = value["machine"]["registry"].as_str().unwrap();
    if cfg!(debug_assertions) {
        assert_eq!(Path::new(shown), named, "a debug build reads the seam");
    } else {
        assert_ne!(
            Path::new(shown),
            named,
            "a release build must not read MIMLYS_RUN_DIR"
        );
    }
    assert!(!named.exists(), "a read command makes nothing");
}
