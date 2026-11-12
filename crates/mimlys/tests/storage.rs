//! `mimlys status --storage` (REQ-WS-013, RFC 013, handoff 013): the store's
//! path, its filesystem and whether it reflinks, the bytes by kind, and the
//! tasks that hold the most; read-only; the numbers from the store's records,
//! with only `tmp/`, the interpreters and the database files measured.
//!
//! The tolerance of the comparison with a walk of the data directory (the
//! `du` of this suite: the sum of the apparent sizes of the regular files):
//! each kind that has a record is compared exactly with the files it stands
//! for; the total is compared with the whole directory less the files that
//! belong to no kind (snapshot manifests, the journals of snapshots being
//! written, each workspace's `workspace.json`, the lock and holder files),
//! which the test names and sums itself; what remains may differ by at most
//! `WAL_SLACK`, because SQLite extends the write-ahead log and its index in
//! steps between two measurements. Apparent sizes, not blocks: a filesystem's
//! block size is an allocation, not a content, and would make every number
//! depend on the file count.

use mimlys_core::spi::Persistence;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

/// Two pages of SQLite's write-ahead-log index (32 KiB each) are the most the
/// database files can differ by between the view and the walk.
const WAL_SLACK: u64 = 64 * 1024;

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

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        assert_eq!(code, 0, "{args:?}: {stdout}\n{stderr}");
        serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"))
    }
}

/// A project with a few files; `extra` makes it differ from the others.
fn scope(root: &Path, name: &str, extra: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "fn a() -> i32 { 0 }\n").unwrap();
    std::fs::write(dir.join("src/old.rs"), "// to be deleted\n").unwrap();
    std::fs::write(
        dir.join("README.md"),
        "the same readme in every project\n".repeat(40),
    )
    .unwrap();
    std::fs::write(dir.join("extra.txt"), extra.repeat(100)).unwrap();
    dir
}

/// The bytes of every regular file under `dir` (apparent size, links not followed).
fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let t = entry.file_type().unwrap();
        if t.is_dir() {
            walk(&entry.path(), out);
        } else if t.is_file() {
            out.push((entry.path(), entry.metadata().unwrap().len()));
        }
    }
}

fn files(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

fn bytes_under(dir: &Path) -> u64 {
    files(dir).iter().map(|f| f.1).sum()
}

/// A store with two completed tasks that share most of their content, a blob
/// each, an interpreter and some temporary files; the test process holds the
/// store open as a writer while the view is taken.
struct Store {
    env: Env,
    ids: Vec<String>,
    writer: mimlys_persist::Store,
}

fn store() -> Store {
    let env = Env::new();
    let scope_a = scope(env.dir.path(), "a", "A");
    let scope_b = scope(env.dir.path(), "b", "B");
    let mut ids = Vec::new();
    for s in [&scope_a, &scope_b] {
        let value = env.json(&["run", "edit", "--scope", s.to_str().unwrap()]);
        assert_eq!(value["task"]["state"], "completed", "{value}");
        ids.push(value["task"]["task_id"].as_str().unwrap().to_owned());
    }
    // Files that have no record, measured on disk.
    std::fs::create_dir_all(env.data.join("interpreters")).unwrap();
    std::fs::write(env.data.join("interpreters/fake.wasm"), vec![7u8; 3000]).unwrap();
    std::fs::write(env.data.join("tmp/junk"), vec![1u8; 700]).unwrap();
    std::fs::create_dir_all(env.data.join("workspaces").join(&ids[0]).join("tmp")).unwrap();
    std::fs::write(
        env.data
            .join("workspaces")
            .join(&ids[0])
            .join("tmp/scratch"),
        vec![2u8; 123],
    )
    .unwrap();
    let writer = mimlys_persist::open(&env.data).unwrap();
    for (id, size) in [(&ids[0], 1500usize), (&ids[1], 900)] {
        let task_id = mimlys_core::ids::TaskId::from_uuid(id.parse().unwrap());
        writer.put_blob(task_id, &vec![9u8; size]).unwrap();
    }
    Store { env, ids, writer }
}

fn database_bytes(data: &Path) -> u64 {
    ["mimlys.db", "mimlys.db-wal", "mimlys.db-shm"]
        .iter()
        .map(|f| std::fs::metadata(data.join(f)).map_or(0, |m| m.len()))
        .sum()
}

fn view(env: &Env) -> Value {
    env.json(&["status", "--storage"])
}

fn kind(view: &Value, name: &str) -> u64 {
    view["bytes"][name]
        .as_u64()
        .unwrap_or_else(|| panic!("{name} in {view}"))
}

/// REQ-WS-013: every kind is what the files it stands for hold, the total is
/// the whole directory less the named files that belong to no kind (within the
/// stated slack), and it is read while another process holds the store.
#[test]
fn req_ws_013_the_storage_view_sums_to_the_disk() {
    let s = store();
    let database_before = database_bytes(&s.env.data);
    let v = view(&s.env);
    let data = &s.env.data;
    assert_eq!(v["path"], data.to_string_lossy().as_ref(), "{v}");

    let trees: u64 = s
        .ids
        .iter()
        .map(|id| bytes_under(&data.join("workspaces").join(id).join("tree")))
        .sum();
    assert_eq!(kind(&v, "working_trees"), trees, "{v}");
    let objects = bytes_under(&data.join("snapshots/objects"));
    assert_eq!(
        kind(&v, "snapshot_objects_exclusive") + kind(&v, "snapshot_objects_shared"),
        objects,
        "{v}"
    );
    assert!(kind(&v, "snapshot_objects_shared") > 0, "{v}");
    assert!(kind(&v, "snapshot_objects_exclusive") > 0, "{v}");
    assert_eq!(kind(&v, "blobs"), bytes_under(&data.join("blobs")), "{v}");
    assert_eq!(kind(&v, "blobs"), 2400, "{v}");
    assert_eq!(kind(&v, "interpreters"), 3000, "{v}");
    assert_eq!(kind(&v, "temporary"), 700 + 123, "{v}");
    // The database files are measured once by the command and once by the test (the
    // store stays open, so nothing checkpoints between): the view's number lies between
    // the measurements before and after the command, and at most the slack beyond.
    let database_after = database_bytes(data);
    assert!(
        kind(&v, "database") >= database_before.min(database_after)
            && kind(&v, "database") <= database_before.max(database_after) + WAL_SLACK,
        "database {} vs {database_before}..{database_after}: {v}",
        kind(&v, "database")
    );
    assert!(database_before > 0);

    // The whole directory, less what belongs to no kind.
    let du: u64 = files(data).iter().map(|f| f.1).sum();
    let belongs_to_no_kind: u64 = files(data)
        .iter()
        .filter(|(p, _)| {
            let rel = p
                .strip_prefix(data)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            rel.starts_with("snapshots/manifests/")
                || rel.starts_with("snapshots/pending/")
                || rel.ends_with("/workspace.json")
                || matches!(
                    rel.as_str(),
                    "mimlys.lock" | "mimlys.holder" | "worker.json" | "toolchain-acls.json"
                )
        })
        .map(|f| f.1)
        .sum();
    assert!(belongs_to_no_kind > 0, "the manifests are in the directory");
    let total = kind(&v, "total");
    assert!(
        (du - belongs_to_no_kind).abs_diff(total) <= WAL_SLACK,
        "du {du} less unlisted {belongs_to_no_kind} vs the view's total {total}: {v}"
    );
    // And the total is the kinds' sum.
    let sum: u64 = [
        "working_trees",
        "snapshot_objects_exclusive",
        "snapshot_objects_shared",
        "blobs",
        "interpreters",
        "database",
        "temporary",
    ]
    .iter()
    .map(|k| kind(&v, k))
    .sum();
    assert_eq!(sum, total);
    drop(s.writer);
}

/// REQ-WS-013: the working trees are the records', not a walk: a file written
/// into a tree after its last snapshot is not in the view (a walk would count
/// it), and the view leaves every file of the directory as it was.
#[test]
fn req_ws_013_the_storage_view_uses_the_records_not_a_walk_of_the_trees() {
    let s = store();
    let before = kind(&view(&s.env), "working_trees");
    let tree = s.env.data.join("workspaces").join(&s.ids[0]).join("tree");
    std::fs::write(
        tree.join("written-after-the-last-snapshot.bin"),
        vec![0u8; 5000],
    )
    .unwrap();
    let files_before = files(&s.env.data);
    let v = view(&s.env);
    assert_eq!(kind(&v, "working_trees"), before, "{v}");
    assert!(
        bytes_under(&tree) >= 5000,
        "a walk would have seen the file"
    );
    // `task show` walks (it shows what the tree holds now); the view does not.
    let shown = s.env.json(&["task", "show", &s.ids[0]]);
    assert!(
        shown["footprint"]["workspace_bytes"].as_u64().unwrap() >= before / 2 + 5000,
        "{shown}"
    );
    let after: Vec<_> = files(&s.env.data)
        .into_iter()
        .filter(|(p, _)| !p.to_string_lossy().ends_with("-shm"))
        .collect();
    let files_before: Vec<_> = files_before
        .into_iter()
        .filter(|(p, _)| !p.to_string_lossy().ends_with("-shm"))
        .collect();
    assert_eq!(after, files_before, "the view wrote nothing");
    drop(s.writer);
}

/// REQ-WS-013: the tasks that hold the most, largest first, each with its
/// state and age and what deleting it frees; the filesystem and the reflink
/// word are shown, in text and in JSON.
#[test]
fn req_ws_013_tasks_by_size_with_state_age_and_the_filesystem() {
    let s = store();
    let v = view(&s.env);
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2, "{v}");
    let totals: Vec<u64> = tasks.iter().map(|t| t["total"].as_u64().unwrap()).collect();
    assert!(totals[0] >= totals[1], "largest first: {totals:?}");
    for t in tasks {
        assert_eq!(t["state"], "completed");
        assert!(t["age_seconds"].as_i64().unwrap() >= 0);
        assert_eq!(
            t["total"].as_u64().unwrap(),
            t["working_tree"].as_u64().unwrap()
                + t["snapshot_objects_exclusive"].as_u64().unwrap()
                + t["blobs"].as_u64().unwrap()
        );
        assert!(s.ids.iter().any(|id| t["task_id"] == id.as_str()));
    }
    // The task with the 1500-byte blob and the extra file of 100 bytes holds more.
    assert_eq!(tasks[0]["blobs"], 1500, "{v}");

    // The filesystem is the platform's name for it, and the word follows the type.
    let name = mimlys_exec::fsinfo::filesystem_name(&s.env.data);
    assert_eq!(
        v["filesystem"]["name"].as_str().map(str::to_owned),
        name,
        "{v}"
    );
    let word =
        serde_json::to_value(mimlys::storage::reflink_of(name.as_deref().unwrap_or(""))).unwrap();
    assert_eq!(v["filesystem"]["reflink"], word, "{v}");
    let (code, text, stderr) = s.env.run(&["status", "--storage"]);
    assert_eq!(code, 0, "{text}\n{stderr}");
    for needle in [
        "store            ",
        "filesystem       ",
        "(reflinks: ",
        "working trees",
        "snapshot objects, exclusive",
        "snapshot objects, shared",
        "blobs",
        "interpreters",
        "database and WAL",
        "temporary",
        "tasks that hold the most",
        "completed",
    ] {
        assert!(text.contains(needle), "{needle:?} in:\n{text}");
    }
    assert!(!text.to_lowercase().contains("locked"), "{text}");
    drop(s.writer);
}

/// REQ-WS-012 (a terminal task's workspace, snapshots, unreferenced objects
/// and blobs go, and nothing of any other task): of two completed tasks that
/// share most of their content and hold a blob each, `task delete` of the
/// first, without `--yes`, shows and removes nothing; with `--yes` it removes
/// the first task's directory, its blob, its manifests and the one object
/// only it referenced, and leaves the second task's tree, blob and manifests
/// and every object the second still references. The next command opens the
/// store, which runs the integrity check (the reference counts equal the
/// manifests', CONS-04), and a rollback of the second task to its base reads
/// the shared objects back.
/// Not verified here, nor by any test: the clause that the next start finishes or undoes a pending snapshot journal. `SnapshotFiles::pending` and `undo` are unit-tested (`rfc_006_journal_finish_or_undo`), but no startup path calls them (handoff 002b slice 3, finding F1).
#[test]
fn req_ws_012_task_delete_removes_the_task_and_what_only_it_held() {
    let s = store();
    let (a, b) = (s.ids[0].clone(), s.ids[1].clone());
    drop(s.writer);
    let data = &s.env.data;
    let snapshots_of = |id: &str| {
        s.env.json(&["task", "show", id])["snapshots"]
            .as_array()
            .unwrap()
            .len()
    };
    let (snapshots_a, snapshots_b) = (snapshots_of(&a), snapshots_of(&b));
    let (manifests, objects) = (
        data.join("snapshots/manifests"),
        data.join("snapshots/objects"),
    );
    assert_eq!(bytes_under(&data.join("blobs")), 1500 + 900);
    assert_eq!(files(&manifests).len(), snapshots_a + snapshots_b);
    let objects_before = files(&objects).len();

    // Shown, asked, not confirmed: nothing goes.
    let shown = s.env.json(&["task", "delete", &a]);
    assert_eq!(shown["deleted"], false, "{shown}");
    assert!(data.join("workspaces").join(&a).exists());
    assert_eq!(bytes_under(&data.join("blobs")), 1500 + 900);
    assert_eq!(files(&manifests).len(), snapshots_a + snapshots_b);
    assert_eq!(files(&objects).len(), objects_before);

    // Confirmed: the first task goes, the second stays.
    let done = s.env.json(&["task", "delete", &a, "--yes"]);
    assert_eq!(done["deleted"], true, "{done}");
    assert!(!data.join("workspaces").join(&a).exists());
    assert_eq!(
        bytes_under(&data.join("blobs")),
        900,
        "only the second blob"
    );
    assert_eq!(files(&manifests).len(), snapshots_b);
    assert_eq!(
        files(&objects).len(),
        objects_before - 1,
        "the one object only the first task referenced is gone, the shared ones stay"
    );
    assert!(data.join("workspaces").join(&b).join("tree").exists());

    // The store is consistent (opening it checked CONS-04) and the shared
    // objects are intact: the second task rolls back to its base from them.
    let shown = s.env.json(&["task", "show", &b]);
    let base = shown["snapshots"][0]["snapshot_id"].as_str().unwrap();
    s.env.json(&["rollback", &b, "--snapshot", base]);
    assert_eq!(
        std::fs::read_to_string(data.join("workspaces").join(&b).join("tree/src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
}

/// REQ-WS-013: an empty store (or none at all) has a view of zeros and
/// creates nothing.
#[test]
fn req_ws_013_the_storage_view_of_no_store_is_empty_and_creates_nothing() {
    let env = Env::new();
    let v = view(&env);
    assert_eq!(kind(&v, "total"), 0, "{v}");
    assert_eq!(v["tasks"].as_array().unwrap().len(), 0);
    let (code, text, _) = env.run(&["status", "--storage"]);
    assert_eq!(code, 0);
    assert!(text.contains("no tasks"), "{text}");
    assert!(!env.data.exists());
}
