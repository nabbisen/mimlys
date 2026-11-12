//! The book's examples that are commands run (REQ-CLI-007, handoffs 013, 010b
//! and 002d): every page listed in `docs/src/SUMMARY.md` is read, and a fenced
//! `sh` block preceded by the comment `<!-- tested -->` is executed line by
//! line against a store with finished, cancelled and held tasks; every
//! `mimlys …` line must exit 0, with `<task>` standing for a finished task.
//! There is no list of page names here: a page that carries the marker but is
//! not in `SUMMARY.md` (so not in the book either), or a marker that opens no
//! block of commands, fails the test. A change to a command that the book
//! shows, or to the book's words for it, that no longer agrees fails here.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

/// The comment that marks a block as tested.
const MARKER: &str = "<!-- tested -->";

/// The commands of each tested block of a Markdown file, in order.
fn tested_blocks(markdown: &str) -> Vec<Vec<String>> {
    let mut blocks = Vec::new();
    let mut lines = markdown.lines();
    while let Some(line) = lines.next() {
        if line.trim() != MARKER {
            continue;
        }
        let fence = lines.next().unwrap_or_default();
        assert!(
            fence.trim() == "```sh",
            "a `{MARKER}` comment is followed by a ```sh block, not {fence:?}"
        );
        let mut commands = Vec::new();
        for line in lines.by_ref() {
            if line.trim() == "```" {
                break;
            }
            let command = line.split('#').next().unwrap_or_default().trim();
            if command.starts_with("mimlys ") {
                commands.push(command.to_owned());
            }
        }
        blocks.push(commands);
    }
    blocks
}

/// The commands of the tested blocks of a Markdown file, in order.
fn tested_commands(markdown: &str) -> Vec<String> {
    tested_blocks(markdown).into_iter().flatten().collect()
}

/// The pages `SUMMARY.md` links, as paths under the book's source directory.
fn summary_pages(summary: &str) -> Vec<String> {
    let mut pages = Vec::new();
    let mut rest = summary;
    while let Some(at) = rest.find("](") {
        rest = &rest[at + 2..];
        let Some(end) = rest.find(')') else { break };
        let target = rest[..end].trim_start_matches("./");
        if target.ends_with(".md") && !target.contains("://") {
            pages.push(target.to_owned());
        }
    }
    pages
}

/// Every Markdown file under `dir`, as paths relative to `base`, with `/`.
fn markdown_files(base: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            markdown_files(base, &path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            let rel = path.strip_prefix(base).unwrap();
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// The commands of the tested blocks of every page of the book at `src`
/// (the directory that holds `SUMMARY.md`), by page. An `Err` names a page
/// that carries the marker and is not read (not in `SUMMARY.md`), a listed
/// page that is missing, or a marker that opens no block with a command.
fn book_commands(src: &Path) -> Result<Vec<(String, Vec<String>)>, String> {
    let summary =
        std::fs::read_to_string(src.join("SUMMARY.md")).map_err(|e| format!("SUMMARY.md: {e}"))?;
    let listed = summary_pages(&summary);
    let mut on_disk = Vec::new();
    markdown_files(src, src, &mut on_disk);
    on_disk.sort();
    let mut read = Vec::new();
    for page in &on_disk {
        let text = std::fs::read_to_string(src.join(page)).map_err(|e| format!("{page}: {e}"))?;
        // A marker is a line that begins or ends with the comment; a page that
        // writes about it inside a code span (this book does) is not one.
        let markers = text
            .lines()
            .filter(|l| l.trim().starts_with(MARKER) || l.trim().ends_with(MARKER))
            .count();
        if markers == 0 {
            continue;
        }
        if !listed.contains(page) {
            return Err(format!(
                "{page} has {markers} `{MARKER}` block(s) and is not read: it is not linked from SUMMARY.md"
            ));
        }
        let blocks = tested_blocks(&text);
        if blocks.len() != markers {
            return Err(format!(
                "{page} has {markers} `{MARKER}` markers and {} blocks read: a marker is not alone on its line",
                blocks.len()
            ));
        }
        if let Some(k) = blocks.iter().position(|b| b.is_empty()) {
            return Err(format!(
                "{page}: tested block {} holds no `mimlys …` command",
                k + 1
            ));
        }
        read.push((page.clone(), blocks.into_iter().flatten().collect()));
    }
    for page in &listed {
        if !on_disk.contains(page) {
            return Err(format!("SUMMARY.md links {page}, which is not a file"));
        }
    }
    if read.is_empty() {
        return Err("no page of the book has a tested block".into());
    }
    Ok(read)
}

/// The book's source directory; `None` in a source tree without the book (a
/// packaged crate carries only its own directory).
fn book_src() -> Option<PathBuf> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/src");
    src.join("SUMMARY.md").is_file().then_some(src)
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

/// REQ-CLI-007: every command in a tested block of every page of the book
/// (the pages `SUMMARY.md` lists) runs and succeeds.
#[test]
fn req_cli_007_the_commands_in_the_books_tested_blocks_run() {
    let Some(src) = book_src() else {
        common::skip(
            "req_cli_007_the_commands_in_the_books_tested_blocks_run",
            "the book is not in this source tree",
        );
        return;
    };
    let pages = book_commands(&src).unwrap_or_else(|e| panic!("{e}"));
    let env = Env::new();
    let finished = env.fill();
    for (page, commands) in &pages {
        for command in commands {
            let words: Vec<String> = command
                .split_whitespace()
                .skip(1)
                .map(|w| w.replace("<task>", &finished))
                .collect();
            let args: Vec<&str> = words.iter().map(String::as_str).collect();
            let (code, stdout, stderr) = env.run(&args);
            assert_eq!(code, 0, "{page}: `{command}` exits 0:\n{stdout}\n{stderr}");
            assert!(
                !stdout.trim().is_empty(),
                "{page}: `{command}` prints something:\n{stderr}"
            );
            assert!(
                !stdout.to_lowercase().contains("locked")
                    && !stderr.to_lowercase().contains("locked"),
                "{page}: `{command}`: {stdout}\n{stderr}"
            );
        }
    }
}

/// REQ-CLI-007: a page with tested blocks that the book does not list, a
/// marker that is not alone on its line, a block without a command, a listed
/// page that is missing and a book without any tested block each fail (the
/// guard of the guard, on a temporary book).
#[test]
fn req_cli_007_a_page_with_tested_blocks_that_is_not_read_fails() {
    let block = "<!-- tested -->\n```sh\nmimlys status\n```\n";
    let book = |summary: &str, pages: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SUMMARY.md"), summary).unwrap();
        for (name, text) in pages {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    };
    // All listed: read, the nested page too; a page without blocks is not read.
    let ok = book(
        "- [A](./a.md)\n- [B](./sub/b.md)\n",
        &[
            ("a.md", block),
            ("sub/b.md", block),
            ("c.md", "no blocks\n"),
        ],
    );
    let read = book_commands(ok.path()).unwrap();
    assert_eq!(
        read.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
        ["a.md", "sub/b.md"]
    );
    // A page with blocks left out of SUMMARY.md.
    let left_out = book("- [A](./a.md)\n", &[("a.md", block), ("sub/b.md", block)]);
    let err = book_commands(left_out.path()).unwrap_err();
    assert!(
        err.contains("sub/b.md") && err.contains("not linked"),
        "{err}"
    );
    // A marker that is not alone on its line is not a block.
    let inline = book(
        "- [A](./a.md)\n",
        &[("a.md", "text <!-- tested -->\n```sh\nmimlys status\n```\n")],
    );
    let err = book_commands(inline.path()).unwrap_err();
    assert!(err.contains("a.md") && err.contains("not alone"), "{err}");
    // A block without a command.
    let empty = book(
        "- [A](./a.md)\n",
        &[("a.md", "<!-- tested -->\n```sh\nls\n```\n")],
    );
    let err = book_commands(empty.path()).unwrap_err();
    assert!(err.contains("holds no"), "{err}");
    // A listed page that is not there.
    let missing = book("- [A](./a.md)\n- [Z](./z.md)\n", &[("a.md", block)]);
    let err = book_commands(missing.path()).unwrap_err();
    assert!(err.contains("z.md"), "{err}");
    // No tested block anywhere.
    let none = book("- [A](./a.md)\n", &[("a.md", "text\n")]);
    assert!(book_commands(none.path()).is_err());
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
