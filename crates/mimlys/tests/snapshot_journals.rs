//! The next start finishes or undoes a pending snapshot journal (REQ-WS-012,
//! external design 16.3; handoff 004c). A crash between a snapshot's journal
//! and its row leaves a journal, perhaps objects and a manifest, and no row;
//! the next command that writes takes the journal (finished when its row
//! exists, undone when not, keeping every object a row references), records
//! `snapshot.journals_recovered`, and prints nothing. The read-only open runs
//! none of this (REQ-PERS-013): `status` says `recovery pending`.
//!
//! The crash is made the way it leaves the files: the real binary completes a
//! task (so the store holds real snapshots), and the test writes the journal,
//! object and manifest files of a snapshot that never got its row.

use mimlys_core::hash::hash_bytes;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

/// A snapshot that never got its row.
const CRASHED: &str = "00000000-0000-7000-8000-00000000abcd";

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
    data: PathBuf,
    scopes: std::cell::Cell<u32>,
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
        Self {
            dir,
            config,
            data,
            scopes: std::cell::Cell::new(0),
        }
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

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        assert_eq!(code, 0, "{args:?}: {stdout}\n{stderr}");
        serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"))
    }

    /// A project with two files, a new one each call.
    fn scope(&self) -> PathBuf {
        let n = self.scopes.get();
        self.scopes.set(n + 1);
        let dir = self.dir.path().join(format!("scope{n}"));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), LIB_BEFORE).unwrap();
        std::fs::write(dir.join("src/old.rs"), "// to be deleted\n").unwrap();
        dir
    }

    /// A task that completed: its tree and its snapshots are real.
    fn completed(&self) -> (String, Vec<String>) {
        let scope = self.scope();
        let value = self.json(&["run", "edit", "--scope", scope.to_str().unwrap()]);
        assert_eq!(value["task"]["state"], "completed", "{value}");
        let id = value["task"]["task_id"].as_str().unwrap().to_owned();
        let view = self.json(&["task", "show", &id]);
        let snapshots = view["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["snapshot_id"].as_str().unwrap().to_owned())
            .collect();
        (id, snapshots)
    }

    /// A command that writes, and so recovers: a held task is the smallest.
    /// Its standard output and error are returned.
    fn writing_start(&self, goal: &str) -> (String, String) {
        let scope = self.scope();
        let (code, stdout, stderr) = self.run(&[
            "task",
            "new",
            goal,
            "--scope",
            scope.to_str().unwrap(),
            "--hold",
        ]);
        assert_eq!(code, 0, "{stdout}\n{stderr}");
        (stdout, stderr)
    }

    fn snapshots(&self) -> PathBuf {
        self.data.join("snapshots")
    }

    fn object(&self, content: &[u8]) -> PathBuf {
        self.snapshots()
            .join("objects")
            .join(hash_bytes(content).as_str())
    }

    fn manifest(&self, id: &str) -> PathBuf {
        self.snapshots()
            .join("manifests")
            .join(format!("{id}.json"))
    }

    fn journal(&self, id: &str) -> PathBuf {
        self.snapshots().join("pending").join(id)
    }

    /// What `snapshot.journals_recovered` events the store holds, as
    /// `(finished, undone, objects_removed)`.
    fn recovered_events(&self) -> Vec<(u64, u64, u64)> {
        let events = self.json(&["events"]);
        events
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["record"]["event"] == "snapshot.journals_recovered")
            .map(|e| {
                let r = &e["record"];
                (
                    r["finished"].as_u64().unwrap(),
                    r["undone"].as_u64().unwrap(),
                    r["objects_removed"].as_u64().unwrap(),
                )
            })
            .collect()
    }

    /// The journal of a snapshot that crashed before its row: the journal
    /// naming `listed`, the object of `content` when `write_object`, and a
    /// manifest.
    fn crash(&self, id: &str, listed: &[&[u8]], objects: &[&[u8]]) {
        let text: Vec<String> = listed
            .iter()
            .map(|c| hash_bytes(c).as_str().to_owned())
            .collect();
        std::fs::write(self.journal(id), text.join("\n")).unwrap();
        for content in objects {
            std::fs::write(self.object(content), content).unwrap();
        }
        std::fs::write(self.manifest(id), r#"{"entries":[]}"#).unwrap();
    }
}

const LIB_BEFORE: &str = "fn a() -> i32 { 0 }\n";
const LIB_AFTER: &str = "fn a() -> i32 { 1 }\n";

fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if name.ends_with("-shm") || name == "mimlys.holder" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let bytes = std::fs::read(&path).unwrap_or_default();
                // SQLite creates an empty `-wal` when it opens a WAL database read-only.
                if name.ends_with("-wal") && bytes.is_empty() {
                    continue;
                }
                out.push((name, bytes));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// REQ-WS-012 (finding F1 of handoff 002b, its probe): a journal with an
/// object and a manifest and no row is taken by the next command that writes:
/// the three files go, a temporary file of an interrupted object write goes,
/// the integrity check of every later start passes, the event records what was
/// done, and nothing is printed.
#[test]
fn req_ws_012_the_next_writing_start_undoes_a_journal_without_a_row() {
    let env = Env::new();
    env.writing_start("first");
    // The crash: no row, so nothing but the next start can reach these files.
    env.crash(CRASHED, &[b"orphan"], &[b"orphan"]);
    let temporary = env
        .snapshots()
        .join("objects")
        .join(format!("{}.tmp-4242", hash_bytes(b"half").as_str()));
    std::fs::write(&temporary, b"ha").unwrap();
    assert!(env.journal(CRASHED).is_file());
    assert!(env.object(b"orphan").is_file());
    assert!(env.manifest(CRASHED).is_file());

    let (stdout, stderr) = env.writing_start("second");

    assert!(!env.journal(CRASHED).exists(), "the journal");
    assert!(!env.object(b"orphan").exists(), "its object");
    assert!(!env.manifest(CRASHED).exists(), "its manifest");
    assert!(!temporary.exists(), "the temporary file");
    for text in [&stdout, &stderr] {
        assert!(
            !text.contains("journal") && !text.contains("recover"),
            "a start that recovers prints nothing about it:\n{stdout}\n{stderr}"
        );
    }
    assert_eq!(env.recovered_events(), [(0, 1, 1)]);
    // A third start has nothing to do: no second event, and the integrity
    // check (which runs at every start) passes.
    env.writing_start("third");
    assert_eq!(env.recovered_events(), [(0, 1, 1)]);
    let metrics = env.json(&["metrics"]);
    assert_eq!(
        metrics["events_by_name"]["snapshot.journals_recovered"], 1,
        "{metrics}"
    );
}

/// REQ-WS-012: a journal whose row exists is finished: the journal goes, the
/// objects and the manifest of the snapshot stay.
#[test]
fn req_ws_012_a_journal_whose_row_exists_is_finished() {
    let env = Env::new();
    let (_, snapshots) = env.completed();
    let last = snapshots.last().unwrap();
    let object = env.object(LIB_AFTER.as_bytes());
    assert!(object.is_file());
    let manifest_before = std::fs::read(env.manifest(last)).unwrap();
    // The crash was after the row: only the journal remained.
    std::fs::write(env.journal(last), hash_bytes(LIB_AFTER.as_bytes()).as_str()).unwrap();

    env.writing_start("next");

    assert!(!env.journal(last).exists(), "the journal goes");
    assert!(object.is_file(), "the object stays");
    assert_eq!(std::fs::read(env.manifest(last)).unwrap(), manifest_before);
    assert_eq!(env.recovered_events(), [(1, 0, 0)]);
    // Everything of the task still reads back.
    let (id, _) = env.completed();
    assert!(env.json(&["task", "show", &id])["snapshots"].is_array());
}

/// REQ-WS-012: a stale journal (a store written by a release that never
/// recovered) lists an object that a later snapshot reused: the object stays
/// and a rollback to that snapshot reads it back; an object of the same
/// journal that no row references goes with the journal and its manifest.
#[test]
fn req_ws_012_a_stale_journal_keeps_an_object_a_later_snapshot_references() {
    let env = Env::new();
    let (id, snapshots) = env.completed();
    let last = snapshots.last().unwrap().clone();
    // The journal lists the content the task's last snapshot holds, and an orphan.
    env.crash(CRASHED, &[LIB_AFTER.as_bytes(), b"orphan"], &[b"orphan"]);
    assert!(env.object(LIB_AFTER.as_bytes()).is_file());

    env.writing_start("next");

    assert!(!env.journal(CRASHED).exists());
    assert!(!env.manifest(CRASHED).exists());
    assert!(!env.object(b"orphan").exists(), "no row references it");
    assert!(
        env.object(LIB_AFTER.as_bytes()).is_file(),
        "a snapshot row references it"
    );
    assert_eq!(env.recovered_events(), [(0, 1, 1)]);

    // The rollback reads the object back.
    let lib = env
        .data
        .join("workspaces")
        .join(&id)
        .join("tree/src/lib.rs");
    std::fs::write(&lib, "changed by hand\n").unwrap();
    let rolled = env.json(&["rollback", &id, "--snapshot", &last]);
    assert!(rolled.is_object(), "{rolled}");
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), LIB_AFTER);
}

/// REQ-WS-012, REQ-PERS-013: the read-only `status` with a pending journal
/// says `recovery pending` and removes nothing; neither does any other command
/// that reads; the next command that writes recovers.
#[test]
fn req_ws_012_the_read_only_status_with_a_pending_journal_says_recovery_pending() {
    let env = Env::new();
    env.writing_start("first");
    env.crash(CRASHED, &[b"orphan"], &[b"orphan"]);
    let before = files(&env.data);

    let (code, stdout, stderr) = env.run(&["status"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("recovery pending: the next command that writes recovers it"),
        "{stdout}"
    );
    let status = env.json(&["status"]);
    assert_eq!(status["store"]["state"], "recovery_pending", "{status}");
    for args in [
        vec!["task", "list"],
        vec!["events"],
        vec!["metrics"],
        vec!["status", "--storage"],
    ] {
        let (code, stdout, stderr) = env.run(&args);
        assert_eq!(code, 0, "{args:?}: {stdout}\n{stderr}");
    }

    assert!(env.journal(CRASHED).is_file(), "the journal stays");
    assert!(env.object(b"orphan").is_file(), "the object stays");
    assert!(env.manifest(CRASHED).is_file(), "the manifest stays");
    assert_eq!(files(&env.data), before, "no file of the store changed");

    env.writing_start("second");
    assert!(!env.journal(CRASHED).exists());
    let (_, stdout, _) = env.run(&["status"]);
    assert!(!stdout.contains("recovery pending"), "{stdout}");
}
