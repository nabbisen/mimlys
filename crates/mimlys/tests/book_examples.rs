//! The book's examples that are commands run (REQ-CLI-007, handoffs 013 and
//! 010b): the Storage page, the "Several projects, several jobs" section of
//! Getting started and the page "How a task runs". A fenced `sh` block preceded by the comment `<!-- tested -->` is
//! executed line by line against a store with finished, cancelled and held
//! tasks; every `mimlys …` line must exit 0, with `<task>` standing for a
//! finished task. A change to a command that the book shows, or to the book's
//! words for it, that no longer agrees fails here.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

/// The commands of the tested blocks of a Markdown file, in order.
fn tested_commands(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = markdown.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "<!-- tested -->" {
            continue;
        }
        let fence = lines.next().unwrap_or_default();
        assert!(
            fence.trim() == "```sh",
            "a `<!-- tested -->` comment is followed by a ```sh block, not {fence:?}"
        );
        for line in lines.by_ref() {
            if line.trim() == "```" {
                break;
            }
            let command = line.split('#').next().unwrap_or_default().trim();
            if command.starts_with("mimlys ") {
                out.push(command.to_owned());
            }
        }
    }
    out
}

/// The Markdown source of a page of the book; `None` in a source tree
/// without the book (a packaged crate carries only its own directory).
fn book(file: &str) -> Option<String> {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/src")
        .join(file);
    std::fs::read_to_string(path).ok()
}

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/m3/edit.json");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n",
                dir.path().join("data").to_string_lossy(),
                script.to_string_lossy()
            ),
        )
        .unwrap();
        Self { dir, config }
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

    fn scope(&self, name: &str) -> String {
        let dir = self.dir.path().join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() -> i32 { 0 }\n").unwrap();
        std::fs::write(dir.join("src/old.rs"), "// to be deleted\n").unwrap();
        dir.to_string_lossy().into_owned()
    }

    /// Two finished tasks, a cancelled one and one held in `created`; returns a finished task's id.
    fn fill(&self) -> String {
        let mut finished = String::new();
        for name in ["a", "b"] {
            let scope = self.scope(name);
            let (code, stdout, stderr) = self.run(&["--json", "run", "edit", "--scope", &scope]);
            assert_eq!(code, 0, "{stdout}\n{stderr}");
            let value: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
            finished = value["task"]["task_id"].as_str().unwrap().to_owned();
        }
        let scope = self.scope("c");
        for hold_then_cancel in [true, false] {
            let (code, stdout, stderr) =
                self.run(&["--json", "task", "new", "held", "--scope", &scope, "--hold"]);
            assert_eq!(code, 0, "{stdout}\n{stderr}");
            if hold_then_cancel {
                let value: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
                let id = value["task"]["task_id"].as_str().unwrap();
                let (code, _, stderr) = self.run(&["task", "cancel", id]);
                assert_eq!(code, 0, "{stderr}");
            }
        }
        finished
    }
}

/// REQ-CLI-007: every command in a tested block of the Storage page and of
/// Getting started runs and succeeds.
#[test]
fn req_cli_007_the_commands_in_the_books_tested_blocks_run() {
    let (Some(storage_page), Some(getting_started), Some(flow_page)) = (
        book("reference/storage.md"),
        book("getting-started.md"),
        book("how-a-task-runs.md"),
    ) else {
        common::skip(
            "req_cli_007_the_commands_in_the_books_tested_blocks_run",
            "the book is not in this source tree",
        );
        return;
    };
    let mut commands = tested_commands(&storage_page);
    let storage = commands.len();
    commands.extend(tested_commands(&getting_started));
    let before_flow = commands.len();
    commands.extend(tested_commands(&flow_page));
    assert!(
        storage >= 4,
        "the Storage page has tested commands: {commands:?}"
    );
    assert!(before_flow > storage, "Getting started has tested commands");
    assert!(
        commands.len() > before_flow,
        "How a task runs has tested commands"
    );
    let env = Env::new();
    let finished = env.fill();
    for command in &commands {
        let words: Vec<String> = command
            .split_whitespace()
            .skip(1)
            .map(|w| w.replace("<task>", &finished))
            .collect();
        let args: Vec<&str> = words.iter().map(String::as_str).collect();
        let (code, stdout, stderr) = env.run(&args);
        assert_eq!(code, 0, "`{command}` exits 0:\n{stdout}\n{stderr}");
        assert!(
            !stdout.trim().is_empty(),
            "`{command}` prints something:\n{stderr}"
        );
        assert!(
            !stdout.to_lowercase().contains("locked") && !stderr.to_lowercase().contains("locked"),
            "`{command}`: {stdout}\n{stderr}"
        );
    }
}

/// REQ-CLI-007: the tested blocks are found (a guard against a marker that
/// stops matching and tests nothing).
#[test]
fn req_cli_007_the_marker_and_the_parser() {
    let text = "intro\n<!-- tested -->\n```sh\nmimlys status --storage   # why\nls\n\nmimlys task list\n```\nmimlys not tested\n";
    assert_eq!(
        tested_commands(text),
        ["mimlys status --storage", "mimlys task list"]
    );
}
