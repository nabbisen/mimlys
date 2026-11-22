//! Output to a closed pipe (REQ-CLI-009, handoff 004e item 3): a command whose standard output
//! is closed by its reader (`mimlys events <task> | head -1`) ends quietly with the exit code
//! its work earned: no panic, nothing on standard error, and `events --follow` ends instead of
//! following into a closed pipe.
//!
//! The pipe is made and closed by the test (`std::io::pipe`, on Unix and on Windows), in two
//! states: **closed from the start** (the reader is dropped before the command is spawned, so
//! that the command's first write finds no reader, by construction and not by timing), and
//! **closed after its first line** (the reader reads one line, as `head -1` does, and goes). The
//! commands are the real `mimlys` binary.

use serde_json::Value;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod common;
use common::run_bounded;

/// How long a command that must end may take: far above its work, far below the suite's bound.
const ENDS_WITHIN: Duration = Duration::from_secs(30);

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
}

/// What a command into a pipe did.
struct Piped {
    code: i32,
    stderr: String,
    /// The lines the reader took before it went (none in the first state).
    lines: Vec<String>,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n",
                data.to_string_lossy()
            ),
        )
        .unwrap();
        Self { dir, config }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = common::mimlys();
        cmd.arg("--config").arg(&self.config).args(args);
        cmd.env("MIMLYS_CONFIG", "");
        cmd
    }

    /// The command with its streams in files: `(exit code, standard output, standard error)`.
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = run_bounded(&mut self.command(args), self.dir.path());
        (out.code(), out.stdout, out.stderr)
    }

    /// A task held in `created` by the real binary: its id.
    fn new_task(&self) -> String {
        let scope = self.dir.path().join("scope");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::write(scope.join("a.rs"), "fn a() {}\n").unwrap();
        let (code, stdout, stderr) = self.run(&[
            "--json",
            "task",
            "new",
            "read me",
            "--scope",
            scope.to_str().unwrap(),
            "--hold",
        ]);
        assert_eq!(code, 0, "{stdout}\n{stderr}");
        let value: Value = serde_json::from_str(stdout.trim()).unwrap();
        value["task"]["task_id"].as_str().unwrap().to_owned()
    }

    /// Runs the command with its standard output a pipe whose reader takes `take` lines and then
    /// goes (0: it is gone before the command starts). Waits for the command to end, within
    /// [`ENDS_WITHIN`]; its standard error is read from a file.
    fn piped(&self, args: &[&str], take: usize) -> Piped {
        let (reader, writer) = std::io::pipe().unwrap();
        let err_path = self.dir.path().join(format!(
            "piped-{}.stderr",
            args.join("_").replace(['/', '\\', ':'], "-")
        ));
        let mut cmd = self.command(args);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(writer))
            .stderr(std::fs::File::create(&err_path).unwrap());
        let mut lines = Vec::new();
        let mut reader = Some(reader);
        if take == 0 {
            drop(reader.take());
        }
        let mut child = cmd.spawn().expect("spawn");
        // The command owns the only write end now: the reader's end ending is what it sees.
        drop(cmd);
        if let Some(r) = reader.take() {
            let mut r = std::io::BufReader::new(r);
            for _ in 0..take {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                lines.push(line.trim_end().to_owned());
            }
            // `head -1` has its line: the reader goes.
            drop(r);
        }
        let started = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            if started.elapsed() > ENDS_WITHIN {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "`mimlys {}` did not end within {} s with its reader gone ({take} line(s) read)",
                    args.join(" "),
                    ENDS_WITHIN.as_secs()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        Piped {
            code: status.code().unwrap_or(-1),
            stderr: std::fs::read_to_string(&err_path).unwrap_or_default(),
            lines,
        }
    }
}

/// Every command of the list, with `take` lines read before the reader goes: each ends with the
/// code it ends with when nothing is closed, with no panic and nothing on standard error.
fn each_command_ends_quietly(take: usize) {
    let env = Env::new();
    let task = env.new_task();
    let commands: Vec<Vec<&str>> = vec![
        vec!["events", &task],
        vec!["task", "show", &task],
        vec!["status"],
        vec!["task", "list"],
        vec!["metrics"],
        vec!["--json", "events", &task],
        vec!["--json", "task", "show", &task],
        vec!["--json", "status"],
        vec!["--json", "task", "list"],
        vec!["--json", "metrics"],
        // Failures: the code is the work's (1), as the text, on standard error or in JSON.
        vec!["task", "show", "no-such-task"],
        vec!["--json", "task", "show", "no-such-task"],
    ];
    for args in commands {
        let (earned, _, earned_stderr) = env.run(&args);
        let piped = env.piped(&args, take);
        let said = args.join(" ");
        assert_eq!(
            piped.code, earned,
            "`mimlys {said}`: the exit code its work earned (reader taking {take} line(s))"
        );
        // A command that fails in text says so on standard error, as always; no other line,
        // and no panic or broken-pipe message, ever.
        assert_eq!(
            piped.stderr, earned_stderr,
            "`mimlys {said}`: standard error is what it is without the pipe (reader taking {take})"
        );
        assert!(
            !piped.stderr.contains("panic") && !piped.stderr.to_lowercase().contains("pipe"),
            "`mimlys {said}`: {}",
            piped.stderr
        );
        if take > 0 {
            assert!(
                piped.lines.len() <= take,
                "`mimlys {said}`: the reader took only what it asked: {:?}",
                piped.lines
            );
        }
    }
}

/// REQ-CLI-009: the reader is gone before the command writes anything.
#[test]
fn req_cli_009_a_command_into_a_pipe_closed_from_the_start_ends_with_its_own_code() {
    each_command_ends_quietly(0);
}

/// REQ-CLI-009: the reader takes the first line and goes (`| head -1`).
#[test]
fn req_cli_009_a_command_into_a_pipe_closed_after_its_first_line_ends_with_its_own_code() {
    each_command_ends_quietly(1);
}

/// REQ-CLI-009: `events <task> --follow | head -1` ends. The task is `created`, so that the
/// follower would otherwise poll for ever. Closed from the start, on every platform: the first
/// write finds no reader.
#[test]
fn req_cli_009_events_follow_into_a_pipe_closed_from_the_start_ends() {
    let env = Env::new();
    let task = env.new_task();
    let piped = env.piped(&["events", &task, "--follow"], 0);
    assert_eq!(piped.code, 0);
    assert_eq!(piped.stderr, "");
}

/// REQ-CLI-009: the reader takes every line of the task's history and goes, and the follower,
/// which has nothing more to write (the task, `created`, says nothing more), ends all the same:
/// on Unix it asks the descriptor, a write is not needed. The reader takes them all, so that
/// the follower cannot learn of the closed pipe from a write that fails: only the poll can end
/// it. On Windows a follower learns that the reader has gone at its next write only, which is
/// not tested here (see the book's page of the command line).
#[cfg(unix)]
#[test]
fn req_cli_009_events_follow_ends_when_the_reader_goes_with_nothing_more_to_write() {
    let env = Env::new();
    let task = env.new_task();
    let (code, history, _) = env.run(&["events", &task]);
    assert_eq!(code, 0);
    let all = history.lines().count();
    assert!(all > 1, "a task has more than one event: {history}");
    let piped = env.piped(&["events", &task, "--follow"], all);
    assert_eq!(piped.code, 0);
    assert_eq!(piped.stderr, "");
    assert_eq!(piped.lines.len(), all, "{:?}", piped.lines);
    assert!(piped.lines[0].contains("task.created"), "{:?}", piped.lines);
}

/// REQ-CLI-009: `events <task> --follow | head -1`: the reader takes the first line and goes.
/// The follower ends, as it does when the reader has taken the last one.
#[cfg(unix)]
#[test]
fn req_cli_009_events_follow_ends_when_the_reader_goes_after_its_first_line() {
    let env = Env::new();
    let task = env.new_task();
    let piped = env.piped(&["events", &task, "--follow"], 1);
    assert_eq!(piped.code, 0);
    assert_eq!(piped.stderr, "");
    assert_eq!(piped.lines.len(), 1, "{:?}", piped.lines);
    assert!(piped.lines[0].contains("task.created"), "{:?}", piped.lines);
}

/// REQ-CLI-009: in JSON `events --follow` writes nothing until the task ends, so that only the
/// descriptor can tell that the reader has gone (Unix).
#[cfg(unix)]
#[test]
fn req_cli_009_events_follow_in_json_ends_when_the_reader_goes() {
    let env = Env::new();
    let task = env.new_task();
    let piped = env.piped(&["--json", "events", &task, "--follow"], 0);
    assert_eq!(piped.code, 0);
    assert_eq!(piped.stderr, "");
}

/// REQ-CLI-009: a command that writes keeps its work: a task is created, the exit code is the
/// work's (0), and the closed pipe is not mentioned.
#[test]
fn req_cli_009_a_writer_into_a_closed_pipe_does_its_work() {
    let env = Env::new();
    let first = env.new_task();
    let scope = env.dir.path().join("scope-two");
    std::fs::create_dir_all(&scope).unwrap();
    std::fs::write(scope.join("b.rs"), "fn b() {}\n").unwrap();
    let piped = env.piped(
        &[
            "task",
            "new",
            "the second",
            "--scope",
            scope.to_str().unwrap(),
            "--hold",
        ],
        0,
    );
    assert_eq!(piped.code, 0, "{}", piped.stderr);
    assert_eq!(piped.stderr, "");
    let (code, stdout, stderr) = env.run(&["--json", "task", "list"]);
    assert_eq!(code, 0, "{stderr}");
    let list: Value = serde_json::from_str(stdout.trim()).unwrap();
    let text = list.to_string();
    assert!(text.contains(&first), "{text}");
    assert!(
        text.matches("the second").count() >= 1,
        "the second task exists after its output went nowhere: {text}"
    );
}

/// REQ-CLI-009: a write that fails for another reason is not a reader that has gone: it is said
/// once, on standard error, and the command does not end in success (Linux: `/dev/full`).
#[cfg(target_os = "linux")]
#[test]
fn req_cli_009_a_write_that_fails_for_another_reason_is_said_once_and_fails() {
    let env = Env::new();
    let mut cmd = env.command(&["status"]);
    let err_path = env.dir.path().join("full.stderr");
    cmd.stdin(Stdio::null())
        .stdout(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        )
        .stderr(std::fs::File::create(&err_path).unwrap());
    let status = cmd.status().unwrap();
    let stderr = std::fs::read_to_string(&err_path).unwrap();
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.contains("cannot write to standard output"),
        "{stderr}"
    );
    assert!(!stderr.contains("panic"), "{stderr}");
}
