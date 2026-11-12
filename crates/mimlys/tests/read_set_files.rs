//! The worker binary and the interpreter modules are admitted as files, not
//! as their directories (RFC 017, handoff 017; REQ-CONF-011): the read sets
//! hold the files, a tool beside mimlys declares its directory, and the
//! refusal at load says which. Pure checks, on every platform; the
//! confinement of a neighbour file is checked per backend in
//! `m6a_host_tools.rs` (Linux), `m6c_host_tools.rs` (macOS) and
//! `m6b_host_tools.rs` (Windows).

use mimlys_exec::interpreters::Configured;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

fn module(dir: &Path, name: &str) -> Configured {
    std::fs::write(dir.join(name), b"module").unwrap();
    Configured {
        language: name.trim_end_matches(".wasm").to_owned(),
        module: dir.join(name),
        sha256: "0".repeat(64),
        script_name: "main.js".into(),
    }
}

fn holds(paths: &[PathBuf], path: &Path) -> bool {
    paths.iter().any(|p| p == path)
}

/// REQ-CONF-011: every worker's read set holds the worker binary's **file**
/// and each configured module's **file**, and neither directory; mimlys's own
/// `interpreters` directory stays. The same for the paths that judge a
/// command (`fixed_read_paths`).
#[test]
fn req_conf_011_the_read_sets_hold_the_worker_and_the_modules_as_files() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("other.txt"),
        b"another file of the folder",
    )
    .unwrap();
    let modules = vec![
        module(folder.path(), "one.wasm"),
        module(folder.path(), "two.wasm"),
    ];
    let worker = mimlys::app::worker_binary();
    let worker_dir = worker.parent().unwrap();
    for deny_network in [true, false] {
        let spec = mimlys::app::confinement_spec(data.path(), &modules, &[], deny_network);
        let read = &spec.read_paths;
        assert!(holds(read, &worker), "the worker's file ({deny_network})");
        assert!(
            !holds(read, worker_dir),
            "not the worker's directory ({deny_network})"
        );
        for m in &modules {
            assert!(holds(read, &m.module), "{} is a file entry", m.language);
        }
        assert!(!holds(read, folder.path()), "not the modules' folder");
        assert!(
            holds(read, &data.path().join("interpreters")),
            "mimlys's own interpreters directory stays"
        );
        assert!(
            !read.iter().any(|p| p == &folder.path().join("other.txt")),
            "nothing else of the folder"
        );
    }
    let fixed = mimlys::app::fixed_read_paths(data.path());
    assert!(holds(&fixed, &worker), "the worker's file judges commands");
    assert!(
        !holds(&fixed, worker_dir),
        "its directory does not: a tool beside mimlys declares it"
    );
}

fn check(config: &str, dir: &Path) -> (i32, String, String) {
    let file = dir.join("config.toml");
    std::fs::write(&file, config).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.arg("--config")
        .arg(&file)
        .arg("check")
        .env("MIMLYS_CONFIG", "")
        .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
    let out = run_bounded(&mut cmd, dir);
    (out.code(), out.stdout, out.stderr)
}

/// REQ-CONF-011, external design §13.5: a host tool whose command lies beside
/// mimlys's worker, and whose declaration names no read path, is refused at
/// load with the directory to declare; naming it, the same configuration
/// loads. (Before RFC 017 the worker's directory was in the fixed read paths
/// and such a command passed without a word.)
#[test]
fn req_conf_011_a_command_beside_the_worker_is_refused_until_its_directory_is_declared() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let tool = PathBuf::from(env!("CARGO_BIN_EXE_mimlys-probe-tool"));
    let tool_dir = tool.parent().unwrap();
    assert_eq!(
        tool_dir,
        Path::new(env!("CARGO_BIN_EXE_mimlys-worker"))
            .parent()
            .unwrap(),
        "the probe tool is beside the worker"
    );
    let base = format!(
        "config_version = 1\n[runtime]\ndata_dir = {:?}\n[host_tools.beside]\ncommand = {:?}\neffect = \"workspace_write\"\ntimeout_ms = 10000\n",
        data.to_string_lossy(),
        tool.to_string_lossy(),
    );
    let (code, stdout, stderr) = check(&base, dir.path());
    assert_eq!(code, 1, "{stdout}\n{stderr}");
    let text = format!("{stdout}{stderr}");
    assert!(text.contains("host_tools.beside.command"), "{text}");
    assert!(
        text.contains("outside the read paths") && text.contains("declare"),
        "{text}"
    );
    assert!(
        text.contains(&tool_dir.file_name().unwrap().to_string_lossy().into_owned()),
        "it names the directory to declare: {text}"
    );
    let declared = format!("{base}read_paths = [{:?}]\n", tool_dir.to_string_lossy());
    let (code, stdout, stderr) = check(&declared, dir.path());
    assert_eq!(code, 0, "{stdout}\n{stderr}");
}
