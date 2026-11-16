//! The M3 demonstration (external design §23; RFC 006): a failed write
//! execution leaves the workspace at its pre-execution snapshot; promotion
//! refuses a conflict and otherwise shows the diff, asks, backs up and
//! applies, and reverses; a terminal task is deleted with its footprint
//! shown; a fork starts from a snapshot with the history attached.
//!
//! The symbolic-link rule in both directions is `workspace::tests::snapshots::
//! req_ws_005_symlinks_both_directions` and `req_ws_002_symlink_rule`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/m3")
        .join(name)
}

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("tools")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "fn a() -> i32 { 0 }\n").unwrap();
    std::fs::write(dir.path().join("src/old.rs"), "// to be deleted\n").unwrap();
    std::fs::write(dir.path().join(".env"), "SECRET=1").unwrap();
    // Creates `/workspace/partial.txt`, writes into it, then exits 1 (a failed write-effect execution).
    let write_then_fail = r#"(module
        (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
        (memory 1) (export "memory" (memory 0))
        (data (i32.const 0) "partial.txt") (data (i32.const 16) "half-written")
        (func (export "_start")
            (drop (call $open (i32.const 3) (i32.const 0) (i32.const 0) (i32.const 11) (i32.const 1) (i64.const 0x40) (i64.const 0) (i32.const 0) (i32.const 100)))
            (i32.store (i32.const 200) (i32.const 16)) (i32.store (i32.const 204) (i32.const 12))
            (drop (call $fd_write (i32.load (i32.const 100)) (i32.const 200) (i32.const 1) (i32.const 300)))
            (call $exit (i32.const 1))))"#;
    std::fs::write(
        dir.path().join("tools/write_then_fail.wasm"),
        wat::parse_str(write_then_fail).unwrap(),
    )
    .unwrap();
    dir
}

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new(script_name: &str) -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n",
                data.path().join("data").to_string_lossy(),
                script(script_name).to_string_lossy()
            ),
        )
        .unwrap();
        Self { data, config }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.data.path());
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

    fn tree(&self, task_id: &str) -> PathBuf {
        self.data
            .path()
            .join("data/workspaces")
            .join(task_id)
            .join("tree")
    }
}

/// External design §23, M3: a failed write execution leaves the workspace
/// at its pre-execution snapshot; the observation says so; a successful
/// write before it persists.
#[test]
fn m3_failed_write_is_rolled_back() {
    let env = Env::new("failed_write.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let tree = env.tree(&id);
    assert_eq!(
        std::fs::read_to_string(tree.join("notes.txt")).unwrap(),
        "kept: a successful write persists\n"
    );
    assert!(
        !tree.join("partial.txt").exists(),
        "the failed execution's write was rolled back"
    );
    let (_, events) = env.json(&["events", &id]);
    let names: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["record"]["event"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"workspace.rollback"), "{names:?}");
    let snapshots: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["record"]["event"] == "workspace.snapshot")
        .map(|e| e["record"]["reason"].as_str().unwrap())
        .collect();
    // The first write's `before` reuses the base and the failed execution's `before` reuses the
    // previous `after`: identical manifests share a record (RFC 006 §2).
    assert_eq!(snapshots, ["base", "after"], "{snapshots:?}");
    assert_eq!(value["snapshots"].as_array().unwrap().len(), 2);
    assert!(value["footprint"]["snapshots"] == 2);
}

/// External design §23, M3: promotion refuses a conflicting file and
/// otherwise shows the diff, needs confirmation, backs up and applies; a
/// patch writes nothing; the promotion is reversed from its backup, which is not a target of
/// `rollback --snapshot` (REQ-WS-006).
#[test]
fn m3_promotion() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    // The patch writes nothing.
    let (code, patch, _) = env.run(&["promote", &id, "--patch"]);
    assert_eq!(code, 0);
    assert!(patch.contains("+fn a() -> i32 { 1 }"), "{patch}");
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
    // Without a terminal and without --yes: the plan, nothing written.
    let (code, value) = env.json(&["promote", &id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["applied"], false);
    assert_eq!(value["plan"]["changes"].as_array().unwrap().len(), 3);
    assert_eq!(value["plan"]["excluded"], serde_json::json!([]));
    assert!(scope.path().join("src/old.rs").exists());
    // A conflict: the user edits a target.
    std::fs::write(scope.path().join("src/lib.rs"), "fn a() -> i32 { 42 }\n").unwrap();
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["kind"], "conflict");
    assert_eq!(
        value["error"]["detail"]["paths"],
        serde_json::json!(["src/lib.rs"])
    );
    std::fs::write(scope.path().join("src/lib.rs"), "fn a() -> i32 { 0 }\n").unwrap();
    // Apply.
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["applied"], true);
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 1 }\n"
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/added.rs")).unwrap(),
        "pub fn added() {}\n"
    );
    assert!(!scope.path().join("src/old.rs").exists());
    assert_eq!(
        std::fs::read_to_string(scope.path().join(".env")).unwrap(),
        "SECRET=1"
    );
    let promotion_id = value["promotion"]["promotion_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, view) = env.json(&["task", "show", &id]);
    assert_eq!(view["promotions"].as_array().unwrap().len(), 1);
    assert_eq!(view["footprint"]["promotions"], 1);
    // The backup is not a target of `rollback --snapshot` (REQ-WS-006): the request is refused and
    // names the promotion's reversal; the task's tree and the project are as they were.
    let backup = view["promotions"][0]["backup_snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    let tree_before = std::fs::read_to_string(env.tree(&id).join("src/lib.rs")).unwrap();
    let (code, refused) = env.json(&["rollback", &id, "--snapshot", &backup]);
    assert_eq!(code, 1, "{refused}");
    assert_eq!(refused["error"]["kind"], "invalid_request", "{refused}");
    let text = refused["error"]["message"].to_string();
    assert!(
        text.contains(&format!("rollback {id} --promotion {promotion_id}")),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(env.tree(&id).join("src/lib.rs")).unwrap(),
        tree_before
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 1 }\n"
    );
    let (_, shown, _) = env.run(&["task", "show", &id]);
    assert!(
        shown.contains("backup") && shown.contains("not a rollback target"),
        "{shown}"
    );
    // Reverse.
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion_id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
    assert!(!scope.path().join("src/added.rs").exists());
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/old.rs")).unwrap(),
        "// to be deleted\n"
    );
    let (_, view) = env.json(&["task", "show", &id]);
    assert!(view["promotions"][0]["reversed_at"].is_string());
    // The user's rollback of the workspace to its base.
    let base = view["snapshots"][0]["snapshot_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, _) = env.json(&["rollback", &id, "--snapshot", &base]);
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(env.tree(&id).join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
}

/// The names of the events of a task, in order.
fn event_names(env: &Env, id: &str) -> Vec<String> {
    let (_, events) = env.json(&["events", id]);
    events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["record"]["event"].as_str().unwrap().to_owned())
        .collect()
}

/// REQ-WS-007, REQ-PERS-007 (external design §14.5, §16.3): a promotion found `applying` at start
/// did not finish. The start restores the project from its backup, records the promotion `failed`
/// with the reason, and it is listed with the task. The crash is made the way it leaves the
/// files: the promotion is applied by the binary, its record is set back to `applying`, and one
/// of the entries it wrote is put back as it was (the apply had not reached it).
#[test]
fn req_ws_007_a_promotion_found_applying_at_start_is_restored_and_listed() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    let promotion_id = value["promotion"]["promotion_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(value["promotion"]["state"], "applied");
    assert!(scope.path().join("src/added.rs").exists());
    // The crash.
    let db = env.data.path().join("data/mimlys.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let changed = conn
            .execute(
                "UPDATE promotions SET state = 'applying', record = json_set(record, '$.state', 'applying') WHERE promotion_id = ?1",
                [&promotion_id],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }
    std::fs::write(scope.path().join("src/old.rs"), "// to be deleted\n").unwrap();
    // The next start that writes.
    let (code, _) = env.json(&["promote", &id]);
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
    assert!(!scope.path().join("src/added.rs").exists());
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/old.rs")).unwrap(),
        "// to be deleted\n"
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join(".env")).unwrap(),
        "SECRET=1"
    );
    let (_, view) = env.json(&["task", "show", &id]);
    assert_eq!(view["promotions"][0]["state"], "failed");
    assert!(
        view["promotions"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("restored from its backup"),
        "{view}"
    );
    assert_eq!(view["footprint"]["promotions"], 0);
    let backups = view["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["reason"] == "backup")
        .count();
    assert_eq!(backups, 1, "the backup is kept");
    let names = event_names(&env, &id);
    assert!(
        names.contains(&"workspace.promotion_failed".to_owned()),
        "{names:?}"
    );
    // A failed promotion cannot be reversed again.
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion_id]);
    assert_eq!(code, 1, "{value}");
    assert_eq!(value["error"]["kind"], "state");
}

/// The object files of the store, set aside; `put_back` returns them. A restore from the backup
/// then fails the way a damaged store makes it fail.
fn set_objects_aside(env: &Env) -> Vec<(PathBuf, PathBuf)> {
    let objects = env.data.path().join("data/snapshots/objects");
    let away = env.data.path().join("away");
    std::fs::create_dir_all(&away).unwrap();
    let mut moved = Vec::new();
    for entry in std::fs::read_dir(&objects).unwrap() {
        let from = entry.unwrap().path();
        let to = away.join(from.file_name().unwrap());
        std::fs::rename(&from, &to).unwrap();
        moved.push((from, to));
    }
    assert!(!moved.is_empty());
    moved
}

fn put_back(moved: Vec<(PathBuf, PathBuf)>) {
    for (from, to) in moved {
        std::fs::rename(to, from).unwrap();
    }
}

/// REQ-WS-007, REQ-PERS-007 (external design §14.5, "There is always a way back"): a promotion
/// found `applying` at start whose restore fails is recorded `failed` with `restored: false`.
/// The start lists it once, with the command; a later start does not try again; `rollback
/// --promotion` restores the project from the kept backup, with the same listing and
/// confirmation, and then the promotion is reversed.
#[test]
fn req_ws_007_a_restore_that_failed_at_start_can_be_restored_by_rollback() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    let promotion_id = value["promotion"]["promotion_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The crash, as in the test above; and a store that cannot give the backup's objects.
    let db = env.data.path().join("data/mimlys.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE promotions SET state = 'applying', record = json_set(record, '$.state', 'applying') WHERE promotion_id = ?1",
            [&promotion_id],
        )
        .unwrap();
    }
    let moved = set_objects_aside(&env);
    // (The command itself is refused: the project changed since the task started.)
    let (code, _, stderr) = env.run(&["promote", &id]);
    assert_eq!(code, 1, "{stderr}");
    let command = format!("mimlys rollback {id} --promotion {promotion_id} to restore the project");
    assert!(stderr.contains(&command), "the start lists it: {stderr}");
    let (_, view) = env.json(&["task", "show", &id]);
    let promotion = &view["promotions"][0];
    assert_eq!(promotion["state"], "failed");
    assert_eq!(promotion["restored"], false);
    assert!(
        promotion["reason"]
            .as_str()
            .unwrap()
            .ends_with(&format!("the backup is kept: run {command}")),
        "{promotion}"
    );
    assert_eq!(
        view["footprint"]["promotions"], 1,
        "its backup is the way back"
    );
    let lib = scope.path().join("src/lib.rs");
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "fn a() -> i32 { 1 }\n",
        "the project was not restored"
    );
    // The command fails while the objects are missing, and the promotion stays restorable.
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion_id, "--yes"]);
    assert_eq!(code, 1, "{value}");
    put_back(moved);
    // A later start does not try again by itself, and does not list it again.
    let (code, _, stderr) = env.run(&["promote", &id]);
    assert_eq!(code, 1, "{stderr}");
    assert!(!stderr.contains(&command), "listed once: {stderr}");
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "fn a() -> i32 { 1 }\n"
    );
    // Without confirmation it lists and changes nothing.
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion_id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["reversed"], false);
    assert!(value["plan"]["entries"].as_array().unwrap().len() >= 2);
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "fn a() -> i32 { 1 }\n"
    );
    // Confirmed, it restores the project from the backup.
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion_id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
    assert!(!scope.path().join("src/added.rs").exists());
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/old.rs")).unwrap(),
        "// to be deleted\n"
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join(".env")).unwrap(),
        "SECRET=1"
    );
    let (_, view) = env.json(&["task", "show", &id]);
    assert!(view["promotions"][0]["reversed_at"].is_string());
    assert_eq!(view["footprint"]["promotions"], 0);
    // Restored once: not again.
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion_id, "--yes"]);
    assert_eq!(code, 1, "{value}");
    assert_eq!(value["error"]["kind"], "state");
}

/// REQ-WS-007 (external design §14.5, "There is always a way back"): an error while applying
/// whose restore fails too. The first change removes an entry; the object that restores it is
/// damaged, so the restore fails. The promotion is `failed` with `restored: false`, the message
/// names the command, and the command restores the project once the store gives the object.
#[cfg(unix)]
#[test]
fn req_ws_007_a_restore_that_failed_after_a_failed_apply_can_be_restored_by_rollback() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new("edit.json");
    let scope = scope();
    std::fs::create_dir_all(scope.path().join("zzz")).unwrap();
    std::fs::write(scope.path().join("zzz/gone.txt"), "to be removed first").unwrap();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    std::fs::remove_file(env.tree(&id).join("zzz/gone.txt")).unwrap();
    let src = scope.path().join("src");
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable = std::fs::File::create(src.join(".probe")).is_ok();
    if writable {
        // Permissions do not bind this user (root): the failure cannot be made this way.
        std::fs::remove_file(src.join(".probe")).unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    // The object that restores `zzz/gone.txt` is damaged.
    let objects = env.data.path().join("data/snapshots/objects");
    let mut damaged = Vec::new();
    for entry in std::fs::read_dir(&objects).unwrap() {
        let path = entry.unwrap().path();
        let original = std::fs::read(&path).unwrap();
        if original == b"to be removed first" {
            std::fs::remove_file(&path).unwrap();
            std::fs::write(&path, b"damaged").unwrap();
            damaged.push((path, original));
        }
    }
    assert_eq!(damaged.len(), 1);
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code, 1, "{value}");
    let (_, view) = env.json(&["task", "show", &id]);
    let promotion = &view["promotions"][0];
    let promotion_id = promotion["promotion_id"].as_str().unwrap().to_owned();
    assert_eq!(promotion["state"], "failed");
    assert_eq!(promotion["restored"], false);
    let message = value["error"]["message"].as_str().unwrap();
    let command = format!(
        "the backup is kept: run mimlys rollback {id} --promotion {promotion_id} to restore the project"
    );
    assert!(
        message.contains("restoring the project from its backup failed too"),
        "{message}"
    );
    assert!(message.ends_with(&command), "{message}");
    assert!(promotion["reason"].as_str().unwrap().ends_with(&command));
    assert!(
        !scope.path().join("zzz/gone.txt").exists(),
        "the entry the apply removed is not back"
    );
    assert_eq!(view["footprint"]["promotions"], 1);
    // The store gives the object again; the command restores the project.
    for (path, original) in damaged {
        std::fs::remove_file(&path).unwrap();
        std::fs::write(path, original).unwrap();
    }
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion_id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        std::fs::read_to_string(scope.path().join("zzz/gone.txt")).unwrap(),
        "to be removed first"
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
    assert!(scope.path().join("src/old.rs").exists());
    assert!(!scope.path().join("src/added.rs").exists());
    let (_, view) = env.json(&["task", "show", &id]);
    assert!(view["promotions"][0]["reversed_at"].is_string());
    assert_eq!(view["footprint"]["promotions"], 0);
}

/// REQ-WS-007 (external design §14.5): an error while applying restores what was applied. A
/// directory the user cannot write to stops the apply after another entry was already removed;
/// the project is as it was, the promotion is `failed`, the backup is kept, and the message says
/// so.
#[cfg(unix)]
#[test]
fn req_ws_007_an_error_while_applying_restores_the_project_and_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new("edit.json");
    let scope = scope();
    std::fs::create_dir_all(scope.path().join("zzz")).unwrap();
    std::fs::write(scope.path().join("zzz/gone.txt"), "to be removed first").unwrap();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    std::fs::remove_file(env.tree(&id).join("zzz/gone.txt")).unwrap();
    let src = scope.path().join("src");
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable = std::fs::File::create(src.join(".probe")).is_ok();
    if writable {
        // Permissions do not bind this user (root): the failure cannot be made this way.
        std::fs::remove_file(src.join(".probe")).unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code, 1, "{value}");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("restored from its backup"), "{message}");
    assert_eq!(
        std::fs::read_to_string(scope.path().join("zzz/gone.txt")).unwrap(),
        "to be removed first",
        "what was applied is restored"
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
    assert!(scope.path().join("src/old.rs").exists());
    assert!(!scope.path().join("src/added.rs").exists());
    let (_, view) = env.json(&["task", "show", &id]);
    assert_eq!(view["promotions"][0]["state"], "failed");
    assert_eq!(view["footprint"]["promotions"], 0);
    assert_eq!(
        view["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["reason"] == "backup")
            .count(),
        1
    );
    let names = event_names(&env, &id);
    let at = |n: &str| names.iter().position(|x| x == n).unwrap();
    assert!(
        at("workspace.promotion_started") < at("workspace.promotion_failed"),
        "{names:?}"
    );
    assert!(!names.contains(&"workspace.promoted".to_owned()));
}

/// REQ-WS-012 (promotion backups): a promoted task has a `backup` snapshot of
/// the targets it replaced; `task delete --yes` removes that snapshot's
/// manifest and every object only the task's snapshots referenced, so no
/// manifest and no object is left, and the project the promotion changed is
/// not touched. (The promotion cannot be reversed afterwards: the backup went
/// with the task, which `task prune` guards against and `task delete` does not.)
#[test]
fn req_ws_012_task_delete_removes_the_promotion_backup() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["applied"], true);
    let (_, view) = env.json(&["task", "show", &id]);
    let reasons: Vec<&str> = view["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["reason"].as_str().unwrap())
        .collect();
    assert!(reasons.contains(&"backup"), "{reasons:?}");
    let manifests = env.data.path().join("data/snapshots/manifests");
    let objects = env.data.path().join("data/snapshots/objects");
    let count = |dir: &Path| std::fs::read_dir(dir).unwrap().count();
    assert_eq!(count(&manifests), reasons.len());
    assert!(count(&objects) > 0);
    let promoted = std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap();
    assert_eq!(promoted, "fn a() -> i32 { 1 }\n");

    let (code, value) = env.json(&["task", "delete", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["deleted"], true);
    assert_eq!(count(&manifests), 0, "the backup's manifest went too");
    assert_eq!(count(&objects), 0, "no object only this task held is left");
    assert!(!env.tree(&id).exists());
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        promoted,
        "the project is not touched by the delete"
    );
}

/// REQ-WS-012 (handoff 006b): a read-only directory under the task's workspace (a toolchain's
/// module cache is made of them) does not keep `task delete` from removing everything: the
/// workspace is gone, and the store holds no manifest and no object of the task, no row and no
/// journal.
#[cfg(unix)]
#[test]
fn req_ws_012_task_delete_removes_a_workspace_holding_read_only_directories() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new("edit.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    // What a toolchain leaves: read-only files in read-only directories, below the tree.
    let cache = env.tree(&id).join("pkg/mod/cache");
    std::fs::create_dir_all(cache.join("v1")).unwrap();
    std::fs::write(cache.join("v1/a.zip"), "a").unwrap();
    std::fs::write(cache.join("b.zip"), "b").unwrap();
    for path in [
        cache.join("v1/a.zip"),
        cache.join("b.zip"),
        cache.join("v1"),
        cache.clone(),
        cache.parent().unwrap().to_path_buf(),
    ] {
        let mode = if path.is_dir() { 0o555 } else { 0o444 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let (code, value) = env.json(&["task", "delete", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["deleted"], true);
    assert!(!env.tree(&id).exists());
    assert!(
        !env.tree(&id).parent().unwrap().exists(),
        "the task's directory is gone"
    );
    let count = |dir: &str| {
        std::fs::read_dir(env.data.path().join("data").join(dir))
            .unwrap()
            .count()
    };
    assert_eq!(
        count("snapshots/manifests"),
        0,
        "no manifest of the task is left"
    );
    assert_eq!(
        count("snapshots/objects"),
        0,
        "no object of the task is left"
    );
    assert_eq!(
        count("snapshots/deleting"),
        0,
        "the deletion's journal is closed"
    );
    let conn = rusqlite::Connection::open(env.data.path().join("data/mimlys.db")).unwrap();
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM snapshots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

/// REQ-WS-007, REQ-SEC-015 (handoff 006b): `rollback --promotion` lists what it will restore and
/// asks, as `promote` does: without a terminal and without `--yes` it changes nothing. A path
/// edited since the promotion is a conflict: `--yes` alone is refused, naming it, and
/// `--overwrite-edited` overwrites it.
#[test]
fn req_ws_007_rollback_of_a_promotion_lists_asks_and_refuses_edits_unless_confirmed() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    let promotion = value["promotion"]["promotion_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Without --yes and without a terminal: the plan, nothing changed.
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["reversed"], false);
    let listed: Vec<(&str, &str)> = value["plan"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["path"].as_str().unwrap(), e["action"].as_str().unwrap()))
        .collect();
    assert_eq!(
        listed,
        [
            ("src/added.rs", "remove"),
            ("src/lib.rs", "restore"),
            ("src/old.rs", "restore"),
        ]
    );
    assert_eq!(value["plan"]["conflicts"], serde_json::json!([]));
    assert!(
        scope.path().join("src/added.rs").exists(),
        "nothing changed"
    );
    let (_, text, _) = env.run(&["rollback", &id, "--promotion", &promotion]);
    assert!(
        text.contains("src/added.rs") && text.contains("not confirmed"),
        "{text}"
    );
    // An edit since the promotion.
    std::fs::write(scope.path().join("src/lib.rs"), "fn a() { edited since }\n").unwrap();
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        value["plan"]["conflicts"],
        serde_json::json!(["src/lib.rs"])
    );
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion, "--yes"]);
    assert_eq!(code, 1, "{value}");
    assert_eq!(value["error"]["kind"], "conflict");
    assert_eq!(
        value["error"]["detail"]["paths"],
        serde_json::json!(["src/lib.rs"])
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() { edited since }\n",
        "the edit is not overwritten"
    );
    assert!(
        scope.path().join("src/added.rs").exists(),
        "nothing was restored"
    );
    let (code, value) = env.json(&[
        "rollback",
        &id,
        "--promotion",
        &promotion,
        "--yes",
        "--overwrite-edited",
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
    assert!(!scope.path().join("src/added.rs").exists());
}

/// RFC 006 §6: deletion needs a terminal task, shows what goes, keeps the audit unless asked.
/// REQ-WS-009 (end to end, `deleted` only by the user's `task delete`, never for a task that is not terminal): the held task refuses the delete with the `state` error, the cancelled one is removed only with `--yes`.
/// REQ-WS-012 (end to end): deletion needs a terminal task, shows the footprint and asks (nothing goes without `--yes`), removes the tree, every manifest and every object, keeps the audit (`task.deleted`) unless `--audit`, and `--audit` removes the task.
/// REQ-OBS-005: after `task delete --yes` the task's events are still listed (`task.deleted`
/// among them); only `task delete --audit` removes the task's record.
/// REQ-PERS-010, REQ-PRIV-006 (external design §16.4): through the binary, a task that is
/// not terminal is refused (`state`) and keeps its workspace, nothing goes without `--yes`,
/// the audit (`task.deleted` among the events) stays after the deletion, and only
/// `--audit` removes the task record.
#[test]
fn m3_delete_with_footprint() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (_, value) = env.json(&[
        "task",
        "new",
        "edit",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&["task", "delete", &id, "--yes"]);
    assert_eq!(code, 1, "not before the task is terminal");
    assert_eq!(value["error"]["kind"], "state");
    env.json(&["task", "cancel", &id]);
    let (code, value) = env.json(&["task", "delete", &id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        value["deleted"], false,
        "without --yes and without a terminal nothing goes"
    );
    assert!(env.tree(&id).exists());
    let (_, list) = env.json(&["task", "list"]);
    assert!(list[0]["footprint"]["workspace_bytes"].as_u64().unwrap() > 0);
    let (code, value) = env.json(&["task", "delete", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["deleted"], true);
    assert!(!env.tree(&id).exists());
    assert!(
        std::fs::read_dir(env.data.path().join("data/snapshots/manifests"))
            .unwrap()
            .next()
            .is_none(),
        "no manifest left"
    );
    assert!(
        std::fs::read_dir(env.data.path().join("data/snapshots/objects"))
            .unwrap()
            .next()
            .is_none(),
        "no object left"
    );
    let (_, events) = env.json(&["events", &id]);
    let names: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["record"]["event"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"task.deleted"),
        "the audit stays: {names:?}"
    );
    let (code, _) = env.json(&["task", "delete", &id, "--yes", "--audit"]);
    assert_eq!(code, 0);
    let (code, value) = env.json(&["task", "show", &id]);
    assert_eq!(code, 1, "{value}");
    assert_eq!(value["error"]["kind"], "not_found");
}

/// REQ-TASK-004: a fork from the source's latest snapshot, with its grants and history.
#[test]
fn m3_fork() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (_, value) = env.json(&[
        "run",
        "edit",
        "--scope",
        scope.path().to_str().unwrap(),
        "--profile",
        "read_only",
        "--grant",
        "tool:write_file",
        "--grant",
        "tool:delete_path",
    ]);
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let source = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&[
        "task",
        "new",
        "continue",
        "--from-task",
        &source[..8],
        "--hold",
    ]);
    assert_eq!(code, 0, "{value}");
    let fork = value["task"]["task_id"].as_str().unwrap().to_owned();
    assert_eq!(value["task"]["forked_from"], source);
    assert_eq!(value["snapshots"][0]["reason"], "fork");
    assert_eq!(
        value["grants"].as_array().unwrap().len(),
        5,
        "the source's grants, copied"
    );
    assert_eq!(
        std::fs::read_to_string(env.tree(&fork).join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 1 }\n"
    );
    assert!(!env.tree(&fork).join("src/old.rs").exists());
    assert_eq!(
        std::fs::read_to_string(env.tree(&source).join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 1 }\n",
        "the source is untouched"
    );
    let (_, shown, _) = env.run(&["task", "show", &fork]);
    assert!(shown.contains("forked    from task"), "{shown}");
}

/// A promoted task, for the tests below: the environment, the project, the task and the
/// promotion's backup snapshot.
fn promoted() -> (Env, tempfile::TempDir, String, String) {
    let env = Env::new("edit.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    let (_, view) = env.json(&["task", "show", &id]);
    let backup = view["promotions"][0]["backup_snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    (env, scope, id, backup)
}

/// REQ-WS-006 (handoff 006c-6): a promotion's backup holds only the project's changed paths, so
/// `task new --from-task ... --from-snapshot <backup>` (and `run`) refuses it as `invalid_request`
/// and creates nothing, as `rollback --snapshot` does; a snapshot of the task is still a source.
#[test]
fn req_ws_006_a_promotions_backup_is_not_a_fork_source_on_the_command_line() {
    let (env, _scope, id, backup) = promoted();
    let listed = |env: &Env| {
        let (_, list) = env.json(&["task", "list"]);
        list.as_array().unwrap().len()
    };
    let before = listed(&env);
    let (code, refused) = env.json(&[
        "task",
        "new",
        "again",
        "--from-task",
        &id,
        "--from-snapshot",
        &backup,
        "--hold",
    ]);
    assert_eq!(code, 1, "{refused}");
    assert_eq!(refused["error"]["kind"], "invalid_request", "{refused}");
    let text = refused["error"]["message"].to_string();
    assert!(
        text.contains("backup of a promotion") && text.contains("source of a fork"),
        "{text}"
    );
    assert_eq!(listed(&env), before, "no task was made");
    let (_, view) = env.json(&["task", "show", &id]);
    let base = view["snapshots"][0]["snapshot_id"].as_str().unwrap();
    assert_ne!(base, backup);
    let (code, forked) = env.json(&[
        "task",
        "new",
        "again",
        "--from-task",
        &id,
        "--from-snapshot",
        base,
        "--hold",
    ]);
    assert_eq!(code, 0, "{forked}");
}

/// REQ-WS-002 (handoff 006c-6): a part of the scope that cannot be read refuses the creation as
/// `invalid_request`, since the scope is the request's input, naming the path; nothing is made.
/// (A task's own tree that cannot be read is `state`.)
#[cfg(unix)]
#[test]
fn req_ws_002_a_scope_that_cannot_be_read_is_an_invalid_request() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new("edit.json");
    let scope = scope();
    let locked = scope.path().join("src/locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("f"), "x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let can_read = std::fs::read_dir(&locked).is_ok();
    let (code, refused) = env.json(&[
        "task",
        "new",
        "go",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if can_read {
        return; // permissions do not bind this user (root): the failure cannot be made this way
    }
    assert_eq!(code, 1, "{refused}");
    assert_eq!(refused["error"]["kind"], "invalid_request", "{refused}");
    let text = refused["error"]["message"].to_string();
    assert!(text.contains("src/locked"), "{text}");
    let (_, list) = env.json(&["task", "list"]);
    assert!(list.as_array().unwrap().is_empty(), "{list}");
}

/// REQ-WS-002 (handoff 006c-6): the same for a name that is not valid UTF-8 (Linux).
#[cfg(target_os = "linux")]
#[test]
fn req_ws_002_a_name_that_is_not_utf8_in_the_scope_is_an_invalid_request() {
    use std::os::unix::ffi::OsStrExt;
    let env = Env::new("edit.json");
    let scope = scope();
    std::fs::write(
        scope
            .path()
            .join(std::ffi::OsStr::from_bytes(b"src/caf\xE9.txt")),
        "x",
    )
    .unwrap();
    let (code, refused) = env.json(&[
        "task",
        "new",
        "go",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    assert_eq!(code, 1, "{refused}");
    assert_eq!(refused["error"]["kind"], "invalid_request", "{refused}");
    let text = refused["error"]["message"].to_string();
    assert!(text.contains("xE9"), "{text}");
}

/// REQ-WS-012 (handoff 006c-6): `task delete` of a task whose deletion leaves files names at most
/// ten of them in its text and counts the rest; the JSON error carries every path. The files are
/// left by putting twelve of the task's objects in the place of non-empty directories.
#[test]
fn req_ws_012_task_delete_names_at_most_ten_paths_and_the_report_all_of_them() {
    for json in [false, true] {
        let env = Env::new("edit.json");
        let scope = scope();
        std::fs::create_dir_all(scope.path().join("extra")).unwrap();
        let mut held = Vec::new();
        for i in 0..12 {
            let text = format!("file number {i}\n");
            std::fs::write(scope.path().join(format!("extra/f{i}.txt")), &text).unwrap();
            held.push(format!(
                "snapshots/objects/{}",
                mimlys_core::hash::hash_bytes(text.as_bytes()).as_str()
            ));
        }
        held.sort();
        let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
        assert_eq!(code, 0, "{value}");
        let id = value["task"]["task_id"].as_str().unwrap().to_owned();
        for rel in &held {
            let path = env.data.path().join("data").join(rel);
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir_all(path.join("held")).unwrap();
        }
        if json {
            let (code, value) = env.json(&["task", "delete", &id, "--yes"]);
            assert_eq!(code, 1, "{value}");
            let paths: Vec<&str> = value["error"]["detail"]["paths"]
                .as_array()
                .unwrap_or_else(|| panic!("{value}"))
                .iter()
                .map(|p| p.as_str().unwrap())
                .collect();
            assert_eq!(paths, held, "every path is in the report");
        } else {
            let (code, _, stderr) = env.run(&["task", "delete", &id, "--yes"]);
            assert_eq!(code, 1, "{stderr}");
            assert!(
                stderr.contains(&format!("task {id} is deleted"))
                    && stderr.contains("12 path(s) could not be removed")
                    && stderr.contains("and 2 more"),
                "{stderr}"
            );
            assert_eq!(
                stderr.matches("snapshots/objects/").count(),
                10,
                "ten paths are named: {stderr}"
            );
        }
    }
}
