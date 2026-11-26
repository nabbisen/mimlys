//! What the tests of the user's copies share (handoff 004d): a WebAssembly module that prints
//! more than an excerpt holds, a scope that holds it, a script that runs it through the
//! `mimlys-testloop` harness, and a look at the blobs of the data directory it leaves.
//! Needs the `test-bins` feature (the harness), which every suite that includes it has.

use super::run_bounded;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const SECRET: &str = "sk-live-0123456789abcdef";

/// What the module prints: far more than an excerpt (LIM-C2) holds, a secret in the middle, a
/// rewritten line (a carriage return that ends no line) and an escape sequence.
pub fn printed() -> String {
    let mut text = String::new();
    for n in 0..800 {
        text.push_str(&format!("line {n:04} of the tool's output\n"));
        if n == 400 {
            text.push_str(&format!("the token is {SECRET} here\n"));
            text.push_str("progress 50%\rprogress 100%\n");
            text.push_str("\u{1b}[31mred\u{1b}[0m\n");
        }
    }
    text
}

/// What the printed text is after the cleaning and the redaction.
pub fn cleaned() -> String {
    printed()
        .replace('\u{1b}', "")
        .replace(SECRET, "[redacted:TOKEN]")
}

/// A WebAssembly module (text) that writes `stdout` to fd 1 and `stderr` to fd 2.
pub fn printer(stdout: &str, stderr: &str) -> Vec<u8> {
    let escape = |text: &str| -> String { text.bytes().map(|b| format!("\\{b:02x}")).collect() };
    let at = stdout.len() as u32 + 16;
    let wat = format!(
        r#"(module
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (memory 1) (export "memory" (memory 0))
        (data (i32.const 0) "{out}")
        (data (i32.const {at}) "{err}")
        (func (export "_start")
            (i32.store (i32.const 60000) (i32.const 0)) (i32.store (i32.const 60004) (i32.const {out_len}))
            (drop (call $fd_write (i32.const 1) (i32.const 60000) (i32.const 1) (i32.const 60100)))
            (i32.store (i32.const 60000) (i32.const {at})) (i32.store (i32.const 60004) (i32.const {err_len}))
            (drop (call $fd_write (i32.const 2) (i32.const 60000) (i32.const 1) (i32.const 60100)))))"#,
        out = escape(stdout),
        err = escape(stderr),
        out_len = stdout.len(),
        err_len = stderr.len(),
    );
    wat::parse_str(wat).unwrap()
}

pub fn scope(stdout: &str, stderr: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tools")).unwrap();
    std::fs::write(dir.path().join("tools/print.wasm"), printer(stdout, stderr)).unwrap();
    dir
}

pub fn script(dir: &Path) -> PathBuf {
    let path = dir.join("script.json");
    let steps = json!([
        {"decision": "act", "tool": "run_wasm",
         "arguments": {"module": "tools/print.wasm", "mode": "read_only"}},
        {"decision": "complete", "summary": "printed"}
    ]);
    std::fs::write(&path, steps.to_string()).unwrap();
    path
}

pub struct Run {
    pub status: std::process::ExitStatus,
    pub report: Option<Value>,
    pub stderr: String,
}

/// The registry of the environment whose data directory is `data`: beside it, in the temporary
/// directory that the test made and removes, so that every command over that data directory
/// names the one registry (handoff 014a-5, item 3).
pub fn registry_of(data: &Path) -> std::path::PathBuf {
    data.parent()
        .expect("a data directory inside the test's directory")
        .join("registry")
}

pub fn testloop(args: &[&str], envs: &[(&str, &str)], data: &Path) -> Run {
    let mut cmd = super::mimlys_testloop_at(&registry_of(data));
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let streams = tempfile::tempdir().expect("a directory for the streams");
    let out = run_bounded(&mut cmd, streams.path());
    Run {
        status: out.status,
        report: serde_json::from_str(&out.stdout).ok(),
        stderr: out.stderr,
    }
}

pub fn run_printer(
    data: &Path,
    scope: &Path,
    script: &Path,
    extra: &[&str],
    envs: &[(&str, &str)],
) -> Run {
    let mut args = vec![
        "--script",
        script.to_str().unwrap(),
        "--data-dir",
        data.to_str().unwrap(),
        "--scope",
        scope.to_str().unwrap(),
        "--secret",
        "TOKEN=sk-live-0123456789abcdef",
    ];
    args.extend_from_slice(extra);
    testloop(&args, envs, data)
}

pub fn blob_files(data: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(data.join("blobs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

pub fn blob_rows(data: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(data.join("mimlys.db")).unwrap();
    let mut stmt = conn
        .prepare("select blob_id from blobs order by blob_id")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The command line over the data directory a run of the harness made: a configuration that
/// names it, and no backend (the commands here run no step).
pub fn mimlys_command(data: &Path, args: &[&str]) -> Command {
    let config = data.parent().unwrap().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\n",
            data.to_string_lossy()
        ),
    )
    .unwrap();
    let mut cmd = super::mimlys_at(&registry_of(data));
    cmd.arg("--config")
        .arg(&config)
        .args(args)
        .env("MIMLYS_CONFIG", "")
        .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
    cmd
}

/// Runs it to its end: the exit code, standard output and standard error.
pub fn mimlys(data: &Path, args: &[&str]) -> (i32, String, String) {
    let out = run_bounded(&mut mimlys_command(data, args), data.parent().unwrap());
    (out.code(), out.stdout, out.stderr)
}

/// A module that writes `out_kib` KiB of `byte` (a WAT data escape: `x`, `\ff`) to standard
/// output and `err_kib` KiB to standard error, a KiB at a time: the exact sizes a bound is tested
/// with, which the text of `printer` cannot carry (a module's data is a few pages).
pub fn printing(out_kib: u32, err_kib: u32, byte: &str) -> Vec<u8> {
    let chunk = byte.repeat(1024);
    let writes = |fd: u32, count: u32| {
        if count == 0 {
            return String::new();
        }
        format!(
            "(local.set $i (i32.const 0))
             (block $done{fd} (loop $again{fd}
                (br_if $done{fd} (i32.ge_u (local.get $i) (i32.const {count})))
                (drop (call $fd_write (i32.const {fd}) (i32.const 2048) (i32.const 1) (i32.const 3000)))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $again{fd})))"
        )
    };
    let wat = format!(
        r#"(module
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (memory 1) (export "memory" (memory 0))
        (data (i32.const 0) "{chunk}")
        (func (export "_start") (local $i i32)
            (i32.store (i32.const 2048) (i32.const 0)) (i32.store (i32.const 2052) (i32.const 1024))
            {out} {err}))"#,
        out = writes(1, out_kib),
        err = writes(2, err_kib),
    );
    wat::parse_str(wat).unwrap()
}

/// A scope that holds `printing(out_kib, err_kib, byte)` as the module `script` runs.
pub fn scope_printing(out_kib: u32, err_kib: u32, byte: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tools")).unwrap();
    std::fs::write(
        dir.path().join("tools/print.wasm"),
        printing(out_kib, err_kib, byte),
    )
    .unwrap();
    dir
}
