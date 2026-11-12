//! Retention (REQ-PERS-010, REQ-PRIV-006; external design §16.4, RFC 006 §6):
//! nothing is deleted by time or by a collector, only by the user's command;
//! a task that is not terminal keeps its recovery point whatever is asked;
//! the audit stays unless `--audit` asks for its removal.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
    data: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let config = dir.path().join("config.toml");
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/m3/edit.json");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n",
                data.to_string_lossy(),
                script.to_string_lossy()
            ),
        )
        .unwrap();
        Self { dir, config, data }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.dir.path());
        (out.code(), out.stdout, out.stderr)
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }

    fn scope(&self, name: &str) -> PathBuf {
        let dir = self.dir.path().join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() -> i32 { 0 }\n").unwrap();
        std::fs::write(dir.join("src/old.rs"), "// to be deleted\n").unwrap();
        dir
    }

    /// A task that ran to `completed`.
    fn completed(&self, name: &str) -> String {
        let scope = self.scope(name);
        let (code, value) = self.json(&["run", "edit", "--scope", scope.to_str().unwrap()]);
        assert_eq!(code, 0, "{value}");
        assert_eq!(value["task"]["state"], "completed", "{value}");
        value["task"]["task_id"].as_str().unwrap().to_owned()
    }

    /// A task held in `created`: not terminal, with its recovery point.
    fn held(&self, name: &str) -> String {
        let scope = self.scope(name);
        let (code, value) = self.json(&[
            "task",
            "new",
            "held",
            "--scope",
            scope.to_str().unwrap(),
            "--hold",
        ]);
        assert_eq!(code, 0, "{value}");
        value["task"]["task_id"].as_str().unwrap().to_owned()
    }

    fn has_workspace(&self, id: &str) -> bool {
        self.data.join("workspaces").join(id).exists()
    }

    /// What the store holds for the task: its state, the number of its
    /// snapshots and the number of its events.
    fn facts(&self, id: &str) -> (String, usize, usize) {
        let (code, view) = self.json(&["task", "show", id]);
        assert_eq!(code, 0, "{view}");
        let (code, events) = self.json(&["events", id]);
        assert_eq!(code, 0, "{events}");
        (
            view["task"]["state"].as_str().unwrap().to_owned(),
            view["snapshots"].as_array().unwrap().len(),
            events.as_array().unwrap().len(),
        )
    }

    /// Makes the task's last change `days` days ago.
    fn age(&self, id: &str, days: i64) {
        let then = mimlys_core::clock::Instant::from_unix_millis(
            mimlys_core::clock::Clock::now(&mimlys_core::clock::SystemClock).unix_millis()
                - days * 86_400_000,
        )
        .unwrap()
        .to_rfc3339();
        let conn = rusqlite::Connection::open(self.data.join("mimlys.db")).unwrap();
        conn.execute(
            "UPDATE tasks SET updated_at = ?1, record = json_set(record, '$.updated_at', ?1) WHERE task_id = ?2",
            [&then, id],
        )
        .unwrap();
    }
}

/// REQ-PERS-010, REQ-PRIV-006 (external design §16.4): no timer and no
/// collector (the retired `workspace.retention_days` key is refused, and `gc`
/// does not exist, `cli.rs`). A terminal task and a task that is not terminal, both older
/// than every retention a timer could have held (400 days; LIM-P3's range
/// ends at 365), keep their workspace, snapshots and events through the
/// commands that open the store for writing (a run, which does the startup
/// recovery) and for reading, and through a `task prune` that is not
/// confirmed. Then the user's command removes what it names and no more: a
/// task that is not terminal is refused, whatever the flags; the audit of a
/// deleted task stays unless `--audit` asks for its removal.
#[test]
fn req_pers_010_nothing_is_deleted_by_age_only_by_command() {
    let env = Env::new();
    // There is no timer to configure: the key of the retired setting is
    // refused, naming the key and the command that deletes.
    let with_timer = env.dir.path().join("with-timer.toml");
    let mut text = std::fs::read_to_string(&env.config).unwrap();
    text.push_str("[workspace]\nretention_days = 30\n");
    std::fs::write(&with_timer, text).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.arg("--config")
        .arg(&with_timer)
        .arg("check")
        .env("MIMLYS_CONFIG", "");
    let out = run_bounded(&mut cmd, env.dir.path());
    assert_eq!(out.code(), 1, "{}\n{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains("workspace.retention_days") && out.stderr.contains("task delete"),
        "{}",
        out.stderr
    );

    let done = env.completed("a");
    let held = env.held("b");
    env.age(&done, 400);
    env.age(&held, 400);
    let done_before = env.facts(&done);
    let held_before = env.facts(&held);
    assert_eq!(done_before.0, "completed");
    assert_eq!(held_before.0, "created");
    assert!(held_before.1 >= 1, "the held task has its recovery point");

    // A writing runtime starts (recovery included); reads; a prune that
    // asks but is not confirmed.
    let other = env.completed("c");
    for args in [
        &["task", "list"][..],
        &["status"][..],
        &["metrics"][..],
        &["task", "prune"][..],
    ] {
        let (code, stdout, stderr) = env.run(args);
        assert_eq!(code, 0, "{args:?}: {stdout}\n{stderr}");
    }
    assert!(env.has_workspace(&done) && env.has_workspace(&held) && env.has_workspace(&other));
    assert_eq!(env.facts(&done), done_before, "the old terminal task");
    assert_eq!(env.facts(&held), held_before, "the old task, not terminal");

    // The only recovery point of a task that is not terminal cannot be
    // deleted, whatever the command says.
    for extra in [&["--yes"][..], &["--yes", "--audit"][..]] {
        let mut args = vec!["task", "delete", held.as_str()];
        args.extend_from_slice(extra);
        let (code, value) = env.json(&args);
        assert_eq!(code, 1, "{args:?}: {value}");
        assert_eq!(value["error"]["kind"], "state", "{value}");
        assert!(env.has_workspace(&held), "{args:?}");
        assert_eq!(env.facts(&held), held_before, "{args:?}");
    }

    // The user's command deletes the terminal task's data and keeps its
    // audit; only `--audit` removes the audit.
    let (code, value) = env.json(&["task", "delete", &done, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert!(!env.has_workspace(&done));
    let (state, snapshots, events) = env.facts(&done);
    assert_eq!((state.as_str(), snapshots), ("completed", 0));
    assert!(
        events > done_before.2,
        "the audit stays and records the deletion"
    );
    assert!(
        env.has_workspace(&other) && env.has_workspace(&held),
        "nothing else went"
    );
    let (code, _) = env.json(&["task", "delete", &done, "--yes", "--audit"]);
    assert_eq!(code, 0);
    let (code, value) = env.json(&["task", "show", &done]);
    assert_eq!(code, 1, "{value}");
    assert_eq!(value["error"]["kind"], "not_found");
    assert!(env.has_workspace(&other) && env.has_workspace(&held));
}
