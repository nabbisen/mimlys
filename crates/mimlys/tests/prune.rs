//! `mimlys task prune` (REQ-WS-014, RFC 013, handoff 013): lists the terminal
//! tasks that match with their footprints, asks once, deletes each as `task
//! delete` does; never a task that is not terminal; keeps a task whose
//! promotion is still reversible; keeps audits unless `--audit`; stops
//! cleanly on a failure.

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

    /// A task that ran to `completed` in a project of its own.
    fn completed(&self, name: &str) -> String {
        let scope = self.scope(name);
        let (code, value) = self.json(&["run", "edit", "--scope", scope.to_str().unwrap()]);
        assert_eq!(code, 0, "{value}");
        assert_eq!(value["task"]["state"], "completed", "{value}");
        value["task"]["task_id"].as_str().unwrap().to_owned()
    }

    /// A task held in `created` (not terminal).
    fn created(&self, name: &str) -> String {
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

    fn cancelled(&self, name: &str) -> String {
        let id = self.created(name);
        let (code, value) = self.json(&["task", "cancel", &id]);
        assert_eq!(code, 0, "{value}");
        id
    }

    /// Whether the task's workspace is still on disk.
    fn has_workspace(&self, id: &str) -> bool {
        self.data.join("workspaces").join(id).exists()
    }

    /// The ids `task list` shows.
    fn listed(&self) -> Vec<String> {
        let (code, value) = self.json(&["task", "list"]);
        assert_eq!(code, 0, "{value}");
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["task_id"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Makes a task's last change `days` days ago.
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

fn index_of(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not in:\n{text}"))
}

/// REQ-WS-014: without `--yes` and without a terminal, prune lists the
/// terminal tasks with their footprints and the total, deletes nothing, and
/// says how to confirm; a task that is not terminal is not in the list.
#[test]
fn req_ws_014_prune_lists_first_and_deletes_nothing_unconfirmed() {
    let env = Env::new();
    let done = env.completed("a");
    let cancelled = env.cancelled("b");
    let held = env.created("c");
    let (code, stdout, stderr) = env.run(&["task", "prune"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains(&done) && stdout.contains(&cancelled),
        "{stdout}"
    );
    assert!(
        !stdout.contains(&held),
        "a task that is not terminal is not listed: {stdout}"
    );
    assert!(stdout.contains("total: 2 task(s)"), "{stdout}");
    assert!(stdout.contains("audit: kept"), "{stdout}");
    assert!(
        stdout.contains("nothing deleted; confirm with --yes"),
        "{stdout}"
    );
    assert_eq!(env.listed().len(), 3, "nothing was deleted");
    let (code, value) = env.json(&["task", "prune"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["confirmed"], false);
    assert_eq!(value["deleted"].as_array().unwrap().len(), 0);
    assert_eq!(value["tasks"].as_array().unwrap().len(), 2);
    assert!(value["total_bytes"].as_u64().unwrap() > 0, "{value}");
}

/// REQ-WS-014: with `--yes`, the listing comes before any deletion, each
/// result is reported, only terminal tasks go, and the audit stays.
#[test]
fn req_ws_014_prune_deletes_only_terminal_tasks_listing_first() {
    let env = Env::new();
    let done = env.completed("a");
    let cancelled = env.cancelled("b");
    let held = env.created("c");
    let (code, stdout, stderr) = env.run(&["task", "prune", "--yes"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let listed_last = index_of(&stdout, "audit: kept");
    let first_deleted = index_of(&stdout, "deleted ");
    assert!(
        listed_last < first_deleted,
        "the plan is shown before anything goes:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("deleted {done}"))
            && stdout.contains(&format!("deleted {cancelled}"))
    );
    assert!(stdout.contains("pruned 2 task(s)"), "{stdout}");
    assert!(!env.has_workspace(&done) && !env.has_workspace(&cancelled));
    assert!(
        env.has_workspace(&held),
        "only the task that is not terminal is left"
    );
    assert_eq!(
        env.listed().len(),
        3,
        "the audit of the deleted tasks stays"
    );
    // The audit stays: the deleted tasks' events are still there.
    let (code, events) = env.json(&["events", &done]);
    assert_eq!(code, 0);
    assert!(
        !events.as_array().unwrap().is_empty(),
        "the audit of {done} stays"
    );
    let (_, again) = env.json(&["task", "prune", "--yes"]);
    assert_eq!(again["tasks"].as_array().unwrap().len(), 0, "{again}");
}

/// REQ-WS-014: `--state` and `--older-than` filter.
#[test]
fn req_ws_014_prune_state_and_older_than_filter() {
    let env = Env::new();
    let old_done = env.completed("a");
    let new_done = env.completed("b");
    let old_cancelled = env.cancelled("c");
    env.age(&old_done, 10);
    env.age(&old_cancelled, 10);

    let (code, value) = env.json(&["task", "prune", "--state", "cancelled"]);
    assert_eq!(code, 0, "{value}");
    let ids = |v: &Value| -> Vec<String> {
        v["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["task_id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(&value), std::slice::from_ref(&old_cancelled), "--state");
    let (_, value) = env.json(&["task", "prune", "--older-than", "7d"]);
    let mut got = ids(&value);
    got.sort();
    let mut want = vec![old_done.clone(), old_cancelled.clone()];
    want.sort();
    assert_eq!(got, want, "--older-than 7d: the task of today is not in");
    let (_, value) = env.json(&[
        "task",
        "prune",
        "--older-than",
        "7d",
        "--state",
        "completed",
    ]);
    assert_eq!(ids(&value), std::slice::from_ref(&old_done), "both");
    let (code, stdout, _) = env.run(&["task", "prune", "--older-than", "30d"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("no terminal task matches"), "{stdout}");
    // Deleting by the filter deletes just those.
    let (code, _) = env.json(&["task", "prune", "--older-than", "7d", "--yes"]);
    assert_eq!(code, 0);
    assert!(!env.has_workspace(&old_done) && !env.has_workspace(&old_cancelled));
    assert!(env.has_workspace(&new_done));
    // A state that is not terminal is refused.
    let (code, _, stderr) = env.run(&["task", "prune", "--state", "halted"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("completed, failed, cancelled"), "{stderr}");
}

/// REQ-WS-014: a task whose promotion can still be reversed is kept and
/// listed with the reason; once the promotion is reversed it goes.
#[test]
fn req_ws_014_prune_keeps_a_task_whose_promotion_is_reversible() {
    let env = Env::new();
    let promoted = env.completed("a");
    let plain = env.completed("b");
    let (code, value) = env.json(&["promote", &promoted, "--yes"]);
    assert_eq!(code, 0, "{value}");
    let promotion = value["promotion"]["promotion_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let (code, stdout, stderr) = env.run(&["task", "prune", "--yes"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let kept_line = stdout.lines().find(|l| l.contains(&promoted)).unwrap();
    assert!(kept_line.contains("kept: promotion reversible"), "{stdout}");
    assert!(stdout.contains(&format!("deleted {plain}")), "{stdout}");
    assert!(!stdout.contains(&format!("deleted {promoted}")), "{stdout}");
    assert!(
        env.has_workspace(&promoted),
        "the promoted task is still there"
    );
    assert!(!env.has_workspace(&plain));
    // Its promotion can still be reversed from the backup.
    let (code, value) = env.json(&["rollback", &promoted, "--promotion", &promotion]);
    assert_eq!(code, 0, "{value}");
    let (code, stdout, _) = env.run(&["task", "prune", "--yes"]);
    assert_eq!(code, 0);
    assert!(stdout.contains(&format!("deleted {promoted}")), "{stdout}");
    assert!(!env.has_workspace(&promoted));
}

/// REQ-WS-014: `--audit` removes the events and records too.
#[test]
fn req_ws_014_prune_audit_removes_the_events_too() {
    let env = Env::new();
    let id = env.completed("a");
    let (code, stdout, _) = env.run(&["task", "prune", "--audit", "--yes"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("audit: the tasks' events and records go too"),
        "{stdout}"
    );
    let (code, events) = env.json(&["events", &id]);
    assert_eq!(code, 0);
    assert!(events.as_array().unwrap().is_empty(), "{events}");
    let (code, _) = env.json(&["task", "show", &id]);
    assert_eq!(code, 1, "the task record is gone");
}

/// REQ-WS-014: a failure midway stops the run: the tasks before it are gone,
/// the rest are untouched and named, the exit code says it did not finish.
/// (A directory the user cannot write makes one deletion fail; Unix, not root.)
#[cfg(unix)]
#[test]
fn req_ws_014_prune_stops_at_a_failure_and_names_the_rest() {
    use std::os::unix::fs::PermissionsExt;
    for json in [true, false] {
        let env = Env::new();
        let first = env.completed("a");
        let second = env.completed("b");
        let third = env.completed("c");
        let tree = env.data.join("workspaces").join(&second).join("tree");
        std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o555)).unwrap();
        let restore = || {
            let _ = std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o755));
        };
        if std::fs::write(tree.join("probe"), b"x").is_ok() {
            restore();
            common::skip(
                "req_ws_014_prune_stops_at_a_failure_and_names_the_rest",
                "this user can write a directory without the permission (root)",
            );
            return;
        }
        if json {
            let (code, value) = env.json(&["task", "prune", "--yes"]);
            restore();
            assert_eq!(code, 1, "{value}");
            assert_eq!(value["failed"]["task_id"], second.as_str(), "{value}");
            assert_eq!(
                value["untouched"],
                serde_json::json!([third.clone()]),
                "{value}"
            );
            assert_eq!(value["deleted"].as_array().unwrap().len(), 1, "{value}");
            assert_eq!(value["deleted"][0]["task_id"], first.as_str(), "{value}");
        } else {
            let (code, stdout, stderr) = env.run(&["task", "prune", "--yes"]);
            restore();
            assert_eq!(code, 1, "{stdout}\n{stderr}");
            assert!(stdout.contains(&format!("deleted {first}")), "{stdout}");
            assert!(stdout.contains(&format!("failed  {second}")), "{stdout}");
            assert!(
                stderr.contains(&format!("prune stopped at {second}"))
                    && stderr.contains(&format!("not touched: {third}")),
                "{stderr}"
            );
        }
        // The first is gone; the third was not touched by the failed run.
        assert!(!env.has_workspace(&first), "deleted before the failure");
        assert!(env.has_workspace(&third), "never touched");
    }
}
