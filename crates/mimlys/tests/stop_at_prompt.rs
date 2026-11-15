//! A stop signal at an approval prompt and at a question prompt (REQ-CLI-005):
//! `mimlys run` on a pseudo-terminal, as a user runs it, waiting for the user's
//! decision or answer. SIGINT, and separately SIGTERM, ends the process with
//! success within LIM-O1, the task stays `waiting_user`, and standard error
//! holds the one line that says so.
//!
//! The verdicts are read from state: the prompt on the terminal (the signal is
//! sent once it is shown, so the process is waiting for the user), the exit of
//! the process, the task as a second process reads it, and standard error. The
//! one bound is the requirement's own: LIM-O1 from the signal to the exit.
//!
//! Unix only: the terminal is a pseudo-terminal and the signals are POSIX
//! (on Windows the file is empty, and no skip is recorded).
#![cfg(unix)]

use serde_json::{Value, json};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;

/// LIM-O1: the time to exit after a stop signal.
const LIM_O1: Duration = Duration::from_millis(mimlys_core::limits::LIM_O1_STOP_MS as u64);

/// How long the prompt may take to appear: the process starts, opens the store
/// and asks the fake backend. No verdict is taken from it.
const PROMPT_BOUND: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug)]
enum Prompt {
    Approval,
    Question,
}

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    /// A fake backend that proposes `write_file` (the profile asks for its
    /// approval), or asks a question, and then completes.
    fn new(prompt: Prompt) -> Self {
        let data = tempfile::tempdir().unwrap();
        let first = match prompt {
            Prompt::Approval => json!({
                "decision": "act", "tool": "write_file",
                "arguments": {"path": "notes/out.txt", "content": "hello"},
            }),
            Prompt::Question => json!({"decision": "ask_user", "question": "Which branch?"}),
        };
        let script = [first, json!({"decision": "complete", "summary": "done"})];
        let script_path = data.path().join("script.json");
        std::fs::write(&script_path, serde_json::to_string(&script).unwrap()).unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n[policy.profiles.workspace]\napproval_tools = [\"write_file\"]\n",
                data.path().join("data").to_string_lossy(),
                script_path.to_string_lossy(),
            ),
        )
        .unwrap();
        Self { data, config }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "");
        cmd
    }

    /// A command that reads, as a second process: its stdout parsed as JSON.
    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = common::run_bounded(&mut self.command(&all), self.data.path());
        assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
        serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr))
    }
}

/// A pseudo-terminal: the terminal side for a child, and a thread that collects
/// what the child writes to it.
struct Terminal {
    slave: std::fs::File,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Terminal {
    fn open() -> Self {
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).expect("a pty master");
        grantpt(&master).expect("grantpt");
        unlockpt(&master).expect("unlockpt");
        let name = ptsname(&master, Vec::new()).expect("the pty's name");
        // The terminal side is opened first, so that nothing written to it is lost.
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name.to_str().expect("a path"))
            .expect("the pty's terminal side");
        let output = Arc::new(Mutex::new(Vec::new()));
        let collected = Arc::clone(&output);
        let mut master = std::fs::File::from(master);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            // Ends when the terminal side is closed (an error on some systems).
            while let Ok(n) = master.read(&mut buf) {
                if n == 0 {
                    break;
                }
                collected.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Self { slave, output }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }
}

/// Runs `mimlys run` on a terminal until its prompt is shown, sends it `signal`,
/// and checks the verdicts of the requirement.
fn stop_at(prompt: Prompt, signal: rustix::process::Signal, name: &str) {
    let env = Env::new(prompt);
    let scope = env.data.path().join("scope");
    std::fs::create_dir_all(&scope).unwrap();
    let terminal = Terminal::open();
    let stderr_path = env.data.path().join("stderr.txt");
    let mut cmd = env.command(&["run", "go on", "--scope", scope.to_str().unwrap()]);
    cmd.stdin(Stdio::from(terminal.slave.try_clone().unwrap()))
        .stdout(Stdio::from(terminal.slave.try_clone().unwrap()))
        .stderr(std::fs::File::create(&stderr_path).unwrap());
    let mut child: Child = cmd.spawn().expect("spawn");
    drop(cmd);

    // The prompt is shown, and the process reads the user's input next.
    let shown = match prompt {
        Prompt::Approval => "[a]pprove",
        Prompt::Question => "Your answer: ",
    };
    let started = Instant::now();
    loop {
        let text = terminal.text();
        if text.contains(shown)
            && text.ends_with(match prompt {
                Prompt::Approval => "> ",
                Prompt::Question => "Your answer: ",
            })
        {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("{name}: exited ({status}) before the prompt:\n{text}");
        }
        if started.elapsed() > PROMPT_BOUND {
            let _ = child.kill();
            panic!("{name}: no prompt within {PROMPT_BOUND:?}:\n{text}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    rustix::process::kill_process(pid, signal).expect("the signal");
    let sent = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if sent.elapsed() > LIM_O1 {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{name}: the process had not exited {LIM_O1:?} after the signal\nterminal:\n{}\nstderr:\n{}",
                terminal.text(),
                std::fs::read_to_string(&stderr_path).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    assert_eq!(
        status.code(),
        Some(0),
        "{name}: exit with success: {status}"
    );
    // The task waits as it did.
    let tasks = env.json(&["task", "list"]);
    assert_eq!(tasks[0]["state"], "waiting_user", "{name}: {tasks}");
    let id = tasks[0]["task_id"].as_str().unwrap();
    // One line, true, in the user's words.
    let expected = match prompt {
        Prompt::Approval => format!(
            "stop signal: {id} is still waiting for your approval; decide later with mimlys approve {id} or mimlys reject {id}"
        ),
        Prompt::Question => format!(
            "stop signal: {id} is still waiting for your answer; answer later with mimlys answer {id} <text>"
        ),
    };
    let stderr = std::fs::read_to_string(&stderr_path).unwrap();
    assert_eq!(stderr.trim_end_matches('\n'), expected, "{name}");
    assert_eq!(stderr.lines().count(), 1, "{name}: {stderr}");
    let on_terminal = terminal.text();
    for word in ["pausing", "pauses", "cancelling"] {
        assert!(
            !on_terminal.contains(word) && !stderr.contains(word),
            "{name}: {word}"
        );
    }
    // And it can be decided later, from another process.
    let pending = env.json(&["task", "show", id]);
    match prompt {
        Prompt::Approval => assert_eq!(pending["pending_approval"]["status"], "pending"),
        Prompt::Question => assert!(!pending["pending_question"].is_null(), "{name}"),
    }
}

/// REQ-CLI-005: SIGINT at an approval prompt.
#[test]
fn req_cli_005_sigint_at_an_approval_prompt_ends_the_process() {
    stop_at(
        Prompt::Approval,
        rustix::process::Signal::INT,
        "SIGINT at an approval prompt",
    );
}

/// REQ-CLI-005: SIGTERM at an approval prompt.
#[test]
fn req_cli_005_sigterm_at_an_approval_prompt_ends_the_process() {
    stop_at(
        Prompt::Approval,
        rustix::process::Signal::TERM,
        "SIGTERM at an approval prompt",
    );
}

/// REQ-CLI-005: SIGINT at a question prompt.
#[test]
fn req_cli_005_sigint_at_a_question_prompt_ends_the_process() {
    stop_at(
        Prompt::Question,
        rustix::process::Signal::INT,
        "SIGINT at a question prompt",
    );
}

/// REQ-CLI-005: SIGTERM at a question prompt.
#[test]
fn req_cli_005_sigterm_at_a_question_prompt_ends_the_process() {
    stop_at(
        Prompt::Question,
        rustix::process::Signal::TERM,
        "SIGTERM at a question prompt",
    );
}
