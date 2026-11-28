//! `mimlys`: the commands of external design §19.2 (REQ-CLI-001 … 005).
//! Exit codes: 0 success, 1 runtime or configuration error, 2 usage error.

// No direct print: the output is written in `mimlys::output` only, with its error handled
// (REQ-CLI-009; see `src/lib.rs` and `clippy.toml`).
#![cfg_attr(
    not(test),
    deny(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_methods)
)]
#![cfg_attr(
    test,
    allow(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_methods)
)]

use clap::{Args, Parser, Subcommand};
use mimlys::app::{App, AppError, ReadApp};
use mimlys::commands::run::NewTask;
use mimlys::config::Config;
use mimlys::output::Output;
use mimlys::prompt::Terminal;
use mimlys::{commands, logging};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "mimlys",
    version,
    about = "A local AI-agent execution and safety runtime: the model proposes, mimlys decides."
)]
struct Cli {
    /// Configuration file (else MIMLYS_CONFIG, else the platform default).
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Print one JSON value instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone)]
struct NewArgs {
    /// What the task should achieve.
    goal: String,
    /// The project directory (the task works on a private copy); the current directory, or a fork's source scope, by default.
    #[arg(long, value_name = "DIR")]
    scope: Option<PathBuf>,
    /// Profile from the policy (default: the policy's default profile).
    #[arg(long, value_name = "NAME")]
    profile: Option<String>,
    /// Extra grant, e.g. `tool:write_file` or `host_read:/path`; repeatable.
    #[arg(long = "grant", value_name = "KIND:SCOPE")]
    grants: Vec<String>,
    /// Step budget within the ceiling.
    #[arg(long, value_name = "N")]
    steps: Option<u32>,
    /// Deadline from now, e.g. `30m`, `2h`.
    #[arg(long, value_name = "DURATION")]
    deadline: Option<String>,
    /// Completion check: `{"tool": NAME, "arguments": {...}}`.
    #[arg(long, value_name = "JSON")]
    completion_check: Option<String>,
    /// Fork: materialize the workspace from this task's snapshot (its scope by default).
    #[arg(long, value_name = "TASK")]
    from_task: Option<String>,
    /// Fork: the snapshot to start from (the source's latest by default).
    #[arg(long, value_name = "SNAPSHOT")]
    from_snapshot: Option<String>,
    /// The model must plan before it acts; you approve the plan before anything acts.
    #[arg(long)]
    plan_first: bool,
    /// The effort level: how much the task may spend (`quick`, `standard` or `deep`; without it,
    /// `standard`, or the level `effort_max` allows if lower; a fork takes its source's). A level
    /// is a budget, never a permission.
    #[arg(long, value_name = "LEVEL", value_parser = ["quick", "standard", "deep"])]
    effort: Option<String>,
    /// Return when the task is running, without following it (a daemon runs it; it needs one).
    #[arg(long)]
    detach: bool,
}

impl NewArgs {
    fn into_new_task(self, hold: bool) -> NewTask {
        NewTask {
            goal: self.goal,
            scope: self.scope.unwrap_or_default(),
            profile: self.profile,
            grants: self.grants,
            steps: self.steps,
            deadline: self.deadline,
            completion_check: self.completion_check,
            hold,
            from_task: self.from_task,
            from_snapshot: self.from_snapshot,
            plan_first: self.plan_first,
            effort: self
                .effort
                .as_deref()
                .and_then(mimlys_core::effort::EffortLevel::parse),
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Create and start a task, follow its events, prompt when it waits.
    Run(NewArgs),
    /// Task control.
    #[command(subcommand)]
    Task(TaskCommand),
    /// Approve the pending action, or the plan a plan-first task waits for; it goes ahead at once.
    Approve {
        /// Task id or a unique prefix.
        task: String,
        /// A note for the model, in your own words.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        /// Return when the task is running, without following it (a daemon runs it; it needs one).
        #[arg(long)]
        detach: bool,
    },
    /// Reject the pending action, or the plan a plan-first task waits for.
    Reject {
        /// Task id or a unique prefix.
        task: String,
        /// A note for the model.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        /// Halt the task instead of continuing.
        #[arg(long)]
        halt: bool,
        /// Return when the task is running, without following it (a daemon runs it; it needs one).
        #[arg(long)]
        detach: bool,
    },
    /// Answer the pending question.
    Answer {
        /// Task id or a unique prefix.
        task: String,
        /// The answer.
        text: String,
        /// Return when the task is running, without following it (a daemon runs it; it needs one).
        #[arg(long)]
        detach: bool,
    },
    /// Issue a grant within the ceiling: `tool NAME`, `host_tool NAME`, `network DOMAIN[:PORT]`.
    Grant {
        /// Task id or a unique prefix.
        task: String,
        /// The grant's kind and scope.
        #[arg(required = true, num_args = 1..)]
        kind: Vec<String>,
    },
    /// Revoke a grant.
    Revoke {
        /// Task id or a unique prefix.
        task: String,
        /// Grant id or a unique prefix.
        grant: String,
    },
    /// Restore the workspace to a snapshot (the latest by default), or reverse a promotion.
    Rollback {
        /// Task id or a unique prefix.
        task: String,
        /// Snapshot id or prefix.
        #[arg(long, value_name = "SNAPSHOT")]
        snapshot: Option<String>,
        /// Reverse this promotion from its backup instead.
        #[arg(long, value_name = "PROMOTION", conflicts_with = "snapshot")]
        promotion: Option<String>,
        /// Reverse the promotion without asking (the only way without a terminal).
        #[arg(long, requires = "promotion")]
        yes: bool,
        /// Also overwrite paths edited since the promotion (otherwise they refuse the reversal).
        #[arg(long, requires = "promotion")]
        overwrite_edited: bool,
    },
    /// Write the workspace's changes to the project after showing the diff and asking.
    Promote {
        /// Task id or a unique prefix.
        task: String,
        /// Apply without asking (the only way without a terminal).
        #[arg(long)]
        yes: bool,
        /// List the changes without the text diffs.
        #[arg(long)]
        summary: bool,
        /// Print a unified patch and write nothing.
        #[arg(long, conflicts_with = "yes")]
        patch: bool,
    },
    /// Show events.
    Events {
        /// Task id or a unique prefix; all tasks when absent.
        task: Option<String>,
        /// Keep printing new events.
        #[arg(long)]
        follow: bool,
        /// Start after this position.
        #[arg(long, default_value_t = 0, value_name = "N")]
        since: u64,
    },
    /// Show derived metrics.
    Metrics {
        /// One task only.
        #[arg(long, value_name = "TASK")]
        task: Option<String>,
    },
    /// Show the runtime's status; with --storage, what the data directory holds instead.
    Status {
        /// Show the store's path, its filesystem, the bytes by kind and the tasks that hold the most.
        #[arg(long)]
        storage: bool,
    },
    /// Validate the configuration and print it.
    Check,
    /// The user's runtimes.
    #[command(subcommand)]
    Runtime(RuntimeCommand),
    /// The local daemon: one process per user and data directory that serves its clients over a
    /// socket (Linux and macOS).
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// The models: whether one acts on this machine.
    #[command(subcommand)]
    Model(ModelCommand),
    /// Interpreter builds for `run_script`.
    #[command(subcommand)]
    Interpreters(InterpretersCommand),
    /// The toolchain paths host tools may read (RFC 011).
    #[command(subcommand)]
    Toolchain(ToolchainCommand),
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Start the daemon, detached: it leads a session of its own and returns once the daemon answers.
    ///
    /// A client that finds a daemon of its data directory sends its requests through it; without
    /// one it runs embedded, as before. The daemon owns the store and runs every client's tasks
    /// in one line. It exits by itself after `[runtime] daemon_idle` with nothing to do.
    Start {
        /// Stay attached to this terminal (for a service manager, or a test).
        #[arg(long)]
        foreground: bool,
        /// What `start` runs in the process it makes: a new session, and the standard streams closed once it listens.
        #[arg(long, hide = true, requires = "foreground")]
        detached: bool,
    },
    /// Stop the daemon: running tasks pause at their next boundary, and are listed with how to resume them.
    Stop,
    /// Stop the daemon, start it again, and resume exactly the tasks the stop paused.
    Restart,
    /// What the daemon is: its process, its socket, its clients and tasks.
    Status {
        /// Also the last lines of its log.
        #[arg(long, value_name = "LINES", num_args = 0..=1, default_missing_value = "20")]
        log: Option<usize>,
    },
}

#[derive(Subcommand)]
enum RuntimeCommand {
    /// Remove the registry's stale entries (a runtime of yours that ended without returning its slot) and list what was removed.
    ///
    /// Every runtime that takes a slot removes them first, so this is for seeing what a crash left, or for tidying when no runtime is about to start. It opens no store and works while other runtimes run.
    Reap,
}

#[derive(Subcommand)]
enum ModelCommand {
    /// Run one small scripted task against a model and record whether it acts on this machine.
    ///
    /// The probe makes a scope of its own under the data directory with one small file, asks the
    /// model for one small edit with three built-in tools and nothing else, and judges the file
    /// itself: the model's word that it is done is never the verdict. The class is `acts`,
    /// `incomplete` (it proposed an action but did not get there), `plans_only` (it never
    /// proposed one) or `no_structured_call`, the first two with how it ended (`not_finished`,
    /// `not_verified`, `asked_user`, `gave_up`), recorded per backend and shown by `status`. An
    /// engine that cannot be reached is said (`unreachable`), exits 1 and is not recorded. The
    /// probe's task and scope are deleted when it ends. Without a name, every local backend is
    /// probed, one after the other; a remote backend is probed only when you name it, and then
    /// only a fixed mini-task is sent, nothing of yours. It writes, so it does not run beside
    /// another runtime of this data directory.
    Probe {
        /// The backend (a name in `[backends]`); every local backend by default.
        backend: Option<String>,
    },
}

#[derive(Subcommand)]
enum ToolchainCommand {
    /// Show the worker's read set and each declared host tool's paths (on Windows: whether each carries the container's permission).
    List,
    /// Windows: grant the worker's AppContainer read access to a toolchain directory, reversibly.
    Grant {
        /// The directory.
        path: String,
        /// Apply without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Windows: remove the permission `toolchain grant` applied to a directory, or with `--all` every permission mimlys or the user added and the container profile (before an uninstall).
    Revoke {
        /// The directory (not with `--all`).
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        path: Option<String>,
        /// Remove every entry of the container (mimlys's own directories included) and delete the profile.
        #[arg(long)]
        all: bool,
        /// Remove without asking.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum InterpretersCommand {
    /// Show the configured interpreters and whether they are installed.
    List,
    /// Verify and install an interpreter module (downloads the shipped build without --from).
    Install {
        /// The language, e.g. `javascript`.
        language: String,
        /// A local module file instead of the download.
        #[arg(long, value_name = "FILE")]
        from: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    /// Create a task (started unless --hold).
    New {
        #[command(flatten)]
        args: NewArgs,
        /// Create only; start later with `task resume`.
        #[arg(long)]
        hold: bool,
    },
    /// Show a task.
    Show {
        /// Task id or a unique prefix.
        task: String,
        /// Also print every argument of the action waiting for approval, in full.
        #[arg(long)]
        arguments: bool,
    },
    /// List tasks.
    List,
    /// Resume a created, paused or halted task.
    Resume {
        /// Task id or a unique prefix.
        task: String,
        /// Raise the step budget (from `halted`).
        #[arg(long, value_name = "N")]
        extend_steps: Option<u32>,
        /// Move the deadline, e.g. `30m` (from `halted`).
        #[arg(long, value_name = "DURATION")]
        extend_deadline: Option<String>,
        /// Return when the task is running, without following it (a daemon runs it; it needs one).
        #[arg(long)]
        detach: bool,
    },
    /// Request a pause at the next step boundary.
    Pause {
        /// Task id or a unique prefix.
        task: String,
    },
    /// Cancel a task.
    Cancel {
        /// Task id or a unique prefix.
        task: String,
    },
    /// Delete terminal tasks in bulk: lists them with their footprints, asks once, then deletes each as `task delete` does.
    Prune {
        /// Only these terminal states (comma-separated: completed, failed, cancelled); all three by default.
        #[arg(long, value_delimiter = ',', value_name = "STATES")]
        state: Vec<String>,
        /// Only tasks whose last change is at least this long ago, e.g. `7d`, `12h`.
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,
        /// Remove the tasks' events and records too.
        #[arg(long)]
        audit: bool,
        /// Delete without asking (the only way without a terminal).
        #[arg(long)]
        yes: bool,
    },
    /// Show what an action produced, whole: its result, what it printed and what it printed as an error.
    ///
    /// The model reads a cut of a long result and of a long output; mimlys keeps the whole of
    /// each for you, and this command shows it. Without a flag it says what the action has (its
    /// result, its standard output, its standard error) with the size of each, whether the model
    /// read a cut of it, and the flag that shows the whole. A tool that printed more than mimlys
    /// keeps (a script's `output_bytes`, 1 MiB by default; a host tool's 4 MiB) has a whole copy
    /// that is itself a cut of what it printed, the first bytes: the index says so and says how
    /// many it kept; and a copy whose text a redaction made longer than mimlys stores (a secret
    /// printed over and over) is cut at its bound, and the index says that the stored copy is
    /// itself cut. With a flag it prints that copy
    /// alone, exactly as stored (the result as JSON), so that it can be piped; on a terminal a
    /// carriage return that ends no line is shown as `\r`, so that no text can hide what
    /// precedes it on its line, and the last line is ended. The action is the task's latest action that
    /// ran unless you name it (an id, or at least four characters of it); the copy is that of its
    /// latest execution unless you give `--attempt N`, and the index names the attempts the action
    /// has, each as it ended, when it has more than one, so that what an earlier attempt printed can be shown. It is a reader: it works while another mimlys process runs a task. It
    /// refuses, with the reason, an action that never ran, an external tool's standard output or
    /// error (it has none), an attempt the action does not have (`not_found`), and a stream the
    /// model read a cut of whose whole copy is not there (the action ran before 0.12, or
    /// `task delete` removed it).
    Output {
        /// Task id or a unique prefix.
        task: String,
        /// Action id or a unique prefix; the task's latest action that ran by default.
        action: Option<String>,
        /// The attempt (execution) to show, from 1; the latest by default. The index lists them.
        #[arg(long, value_name = "N")]
        attempt: Option<u32>,
        /// Print the whole standard output.
        #[arg(long, group = "copy")]
        stdout: bool,
        /// Print the whole standard error.
        #[arg(long, group = "copy")]
        stderr: bool,
        /// Print the whole result, as JSON.
        #[arg(long, group = "copy")]
        result: bool,
    },
    /// Delete a terminal task's workspace, snapshots and blobs; the audit stays unless --audit.
    Delete {
        /// Task id or a unique prefix.
        task: String,
        /// Remove the task's events and records too.
        #[arg(long)]
        audit: bool,
        /// Delete without asking (the only way without a terminal).
        #[arg(long)]
        yes: bool,
    },
}

/// The stack of the thread that runs the command, in bytes.
///
/// The commands' futures are large and the runtime polls them on the calling
/// thread (`block_on`), and the parse of the command line is large too in a
/// debug build, so the stack the operating system gives the main thread (1 MiB
/// on Windows, usually 8 MiB on Linux and macOS) is not a bound the code should
/// rest on. Measured on Linux in a debug build, the heaviest path of the tests
/// (`m5_mcp`, the restarts of an MCP server) needs about 985 KiB: it fails at
/// 980 KiB and passes at 990. 8 MiB is more than eight times that. A stack is
/// address space reserved up front and committed page by page as it is used, so
/// the size costs nothing until it is touched. A debug build can run the
/// thread on less (`command_stack_bytes`), which the tests use to keep that
/// margin: they run at a quarter, 2 MiB.
const RUNTIME_STACK_BYTES: usize = 8 * 1024 * 1024;

/// The stack of the command thread as this process uses it.
///
/// A debug build reads `MIMLYS_DEBUG_STACK_DIVISOR` (a whole number from 1 to
/// 1024) and gives the thread `RUNTIME_STACK_BYTES` divided by it. The test
/// harness sets 4, so that a growth of the heaviest path past a quarter of the
/// stack fails in CI before it can fail anywhere else, and the stack itself
/// keeps a margin of four over what the tests accept. A release build does not
/// read the variable (the code is not compiled), and a value outside the range
/// is ignored: the full stack.
fn command_stack_bytes() -> usize {
    #[cfg(debug_assertions)]
    if let Some(divisor) = std::env::var("MIMLYS_DEBUG_STACK_DIVISOR")
        .ok()
        .and_then(|text| text.trim().parse::<usize>().ok())
        .filter(|n| (1..=1024).contains(n))
    {
        return RUNTIME_STACK_BYTES / divisor;
    }
    RUNTIME_STACK_BYTES
}

fn main() -> ExitCode {
    // REQ-CONF-012: first thing, before any thread exists (the async runtime
    // below starts them). Fail closed: the guarantee is stated in the book.
    if let Err(e) = mimlys_exec::confine::harden_process() {
        mimlys::output::diag(format!("mimlys: cannot mark the process non-dumpable: {e}"));
        return ExitCode::from(1);
    }
    // REQ-SEC-011: replaces the default hook, whose message goes to standard
    // error unredacted. The secrets are added once the configuration loaded.
    mimlys_exec::panic_hook::install("mimlys");
    // Everything else runs on a thread with the stack stated above, and `main`
    // returns its exit code. The thread is named `main`: it is the program's
    // main flow, and the panic hook's line names the thread that panicked.
    let spawned = std::thread::Builder::new()
        .name("main".into())
        .stack_size(command_stack_bytes())
        .spawn(run_process);
    match spawned {
        // The thread owns the runtime and everything the command opened: they
        // are dropped on it, before `join` returns, as they were in `main`.
        Ok(thread) => match thread.join() {
            Ok(code) => code,
            // A panic ended the thread; the hook has written its line. The
            // code is the one Rust gives a panic on the main thread.
            Err(_) => ExitCode::from(101),
        },
        Err(e) => {
            Output::new(false).error(&AppError::Io {
                detail: format!("cannot start the main thread: {e}"),
            });
            ExitCode::from(1)
        }
    }
}

/// Parses the command line and runs the command on a new runtime, which this
/// thread drives. Returns the exit code.
fn run_process() -> ExitCode {
    // A usage error or `--help` ends the process here, as before.
    let cli = Cli::parse();
    let out = Output::new(cli.json);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            out.error(&AppError::Io {
                detail: format!("cannot start the async runtime: {e}"),
            });
            return ExitCode::from(1);
        }
    };
    let code = match runtime.block_on(run(cli, &out)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            out.error(&e);
            ExitCode::from(1)
        }
    };
    // A reader that has gone is not a failure (REQ-CLI-009): the exit code is the work's. A write
    // that failed for another reason (a full disk) is said once, on standard error, and a command
    // whose output did not arrive does not end in success.
    match mimlys::output::write_failure() {
        Some(detail) => {
            mimlys::output::diag(mimlys::messages::error_line(
                "internal",
                &mimlys::messages::cannot_write_output(&detail),
            ));
            ExitCode::from(1)
        }
        None => code,
    }
}

async fn run(cli: Cli, out: &Output) -> Result<(), AppError> {
    let loaded = Config::load(Config::locate(cli.config.as_deref()).as_deref())?;
    mimlys_exec::panic_hook::set_secrets(&loaded.secrets);
    // Debug builds only (REQ-SEC-011's test): a panic after the configuration
    // loaded, whose message is the variable's text.
    #[cfg(debug_assertions)]
    if let Ok(text) = std::env::var("MIMLYS_DEBUG_PANIC_AFTER_CONFIG") {
        panic!("{text}");
    }
    // The daemon's own process logs to its file, which it opens itself.
    let daemon_process = matches!(
        &cli.command,
        Command::Daemon(DaemonCommand::Start {
            foreground: true,
            ..
        })
    );
    if !daemon_process && let Some(warning) = logging::init(&loaded.config.logging, &loaded.secrets)
    {
        mimlys::output::diag(warning);
    }
    if let Command::Check = &cli.command {
        return commands::check::run(&loaded, out);
    }
    if let Command::Daemon(daemon) = cli.command {
        return run_daemon(daemon, loaded, out).await;
    }
    // The registry of the user's runtimes belongs to no store: no app is opened for it.
    if let Command::Runtime(RuntimeCommand::Reap) = &cli.command {
        return commands::runtime::reap(out);
    }
    // REQ-PERS-013: a command that only reads opens the store as a reader,
    // without the lock, and runs no recovery, reaping or migration. It cannot
    // call a writing method: its handle has none.
    if let Some(read) = ReadCommand::of(&cli.command) {
        let app = ReadApp::open(loaded)?;
        // `promote --patch` plans a promotion: of a task that holds a word this release does not
        // know, it refuses as the commands that change the task do (REQ-MAIN-006).
        if let Some(task) = changes_task(&cli.command) {
            let id = app.resolve_task(task)?;
            mimlys_core::newer::refuse_if_newer(app.reader.persistence(), id)?;
        }
        return dispatch_read(&app, read, out).await;
    }
    // A daemon of this store serves the commands that run tasks (REQ-ARCH-009): their requests go
    // through it, and this process opens no store for writing. Without one, the command runs
    // embedded, as before. `--detach` needs a daemon, and is refused without one, before anything
    // is created.
    let detach = detach_of(&cli.command);
    let served = route(&loaded, &cli.command)?;
    if detach && served.is_none() {
        return Err(AppError::Io {
            detail: mimlys::messages::DETACH_NEEDS_A_DAEMON.to_owned(),
        });
    }
    // Every writer is refused while another process holds the store, naming it (LIM-O3): the
    // refusal is made where the store is opened (`App::open`), by one function.
    let mut app = match served {
        Some(transport) => App::open_served(loaded, transport, None)?,
        None => App::open(loaded)?,
    };
    app.detach = detach;
    // A task runs on the backend named `default`, and no command picks another when there is
    // none (REQ-CFG-007; RFC 015, Amendment 1): this is said before anything is created or run.
    // Every command that can lead to a step says it, `grant` and `revoke` included (`Steps`).
    let steps = Steps::of(&cli.command);
    if steps.needs_default() {
        app.loaded.config.default_backend()?;
    }
    // A command that would change or run a task refuses a task that holds a word a newer release
    // wrote and this one does not know, before it asks, contacts a backend or writes anything
    // (REQ-MAIN-006; external design §22). The commands are those of `changes_task`. It follows the
    // check that a backend named `default` exists, which is the configuration's, and comes before
    // the one that it can answer, which is not a reason to answer for a task that is refused.
    if let Some(task) = changes_task(&cli.command) {
        let id = app.resolve_task(task)?;
        mimlys_core::newer::refuse_if_newer(app.reader.persistence(), id)?;
    }
    // The backend the task of this command runs on: the one its level's envelope names, found by
    // that name (REQ-TASK-013). A command opens a connection to that backend and to no other.
    let target = target_of(&app, &cli.command);
    let backend = target
        .as_ref()
        .map_or(mimlys::backends::DEFAULT, |t| t.backend.as_str());
    if steps.starts_a_task() {
        app.backend_ready()?;
        if backend != mimlys::backends::DEFAULT {
            app.backend_ready_for(backend)?;
        }
    }
    // REQ-SEC-010: a backend that the start of this command's task is refused is not contacted,
    // not even to be probed; the command goes on and says why (the creation is refused, or the
    // task halts `policy`).
    let forbidden = start_is_forbidden(&app, &cli.command, backend);
    if steps.contacts_backend()
        && !forbidden
        && let Some(notice) = app.probe_backend_named(backend).await?
    {
        out.line(notice);
    }
    // A creation that starts a task, at any level and with or without `--effort`, is refused
    // when its backend does not answer, before anything is made (REQ-MDL-010; external design
    // §20).
    if steps.contacts_backend()
        && !forbidden
        && let Some(target) = &target
        && target.refuses_unreachable()
        && let Some(probe) = app.backend_probe_of(backend)
        && !probe.reachable
        && let Some(creation) = target.creation
    {
        return Err(AppError::Io {
            detail: mimlys::messages::level_backend_unreachable(
                creation
                    .backend_chosen_by_level
                    .then(|| creation.level.name()),
                backend,
                probe.detail.as_deref().unwrap_or("no answer"),
            ),
        });
    }
    // A context below the need is a warning on standard error, never an error:
    // the run goes on (REQ-MDL-003).
    if steps.contacts_backend()
        && let Some(warning) = app.context_warning_for(backend)
    {
        mimlys::output::diag(warning);
    }
    // A backend that serves another model than the task last ran with: the task goes on, and
    // the start says both models (external design §20).
    if let Some(task) = target.as_ref().and_then(|t| t.task.as_ref())
        && let Ok(Some(change)) = app.client.model_change(task.task_id)
    {
        mimlys::output::diag(mimlys::messages::model_changed_notice(
            &change.backend,
            &change.was,
            &change.now,
        ));
    }
    let interactive =
        !cli.json && std::io::stdin().is_terminal() && mimlys::output::stdout_is_terminal();
    // Standard input is read on a thread of its own, so that a stop signal at a
    // prompt ends the wait (REQ-CLI-005).
    let mut terminal = Terminal::on_standard_streams(interactive);
    let result = dispatch(&app, cli.command, &mut terminal, out).await;
    let closed = app.close();
    result.and(closed)
}

/// `--detach`: return when the task is running, without following it.
fn detach_of(command: &Command) -> bool {
    match command {
        Command::Run(args) => args.detach,
        Command::Task(TaskCommand::New { args, .. }) => args.detach,
        Command::Task(TaskCommand::Resume { detach, .. })
        | Command::Approve { detach, .. }
        | Command::Reject { detach, .. }
        | Command::Answer { detach, .. } => *detach,
        _ => false,
    }
}

/// Whether the daemon serves the command: every one that goes through the control API and the
/// store's readers, and `model probe` (a request the daemon runs, on the store it holds: the
/// probe builds a runtime of its own over the store, which only the process that holds the lock
/// may do, REQ-PERS-014). Not `toolchain grant` and `toolchain revoke` (they write this machine's
/// permissions and the store's record of them): those need the store itself, and are refused,
/// naming the daemon, while one holds it.
#[cfg(unix)]
fn servable(command: &Command) -> bool {
    !matches!(
        command,
        Command::Toolchain(ToolchainCommand::Grant { .. } | ToolchainCommand::Revoke { .. })
    )
}

/// The transport to the daemon of this store, when one runs and serves the command; an error
/// when one runs and refuses this client (another version).
#[cfg(unix)]
fn route(
    loaded: &mimlys::config::Loaded,
    command: &Command,
) -> Result<Option<std::sync::Arc<dyn mimlys::client::Transport>>, AppError> {
    use mimlys::daemon::command as lifecycle;
    if !servable(command) {
        return Ok(None);
    }
    let Some(daemon) = lifecycle::find(loaded, false)? else {
        return Ok(None);
    };
    if !daemon.usable() {
        let detail = match &daemon.answer.refusal {
            Some(refusal)
                if daemon.hello.runtime_version == mimlys::daemon::wire::runtime_version() =>
            {
                refusal.message.clone()
            }
            _ => mimlys::messages::daemon_version_refused(
                mimlys::daemon::wire::runtime_version(),
                &daemon.hello.runtime_version,
            ),
        };
        return Err(AppError::Io { detail });
    }
    drop(daemon);
    let (run_dir, data_dir) = lifecycle::places(loaded)?;
    Ok(Some(mimlys::daemon::transport::Socket::new(
        run_dir,
        data_dir,
        loaded.clone(),
    )))
}

/// Windows has no daemon in this release: every command runs embedded.
#[cfg(not(unix))]
fn route(
    _loaded: &mimlys::config::Loaded,
    _command: &Command,
) -> Result<Option<std::sync::Arc<dyn mimlys::client::Transport>>, AppError> {
    Ok(None)
}

/// The task a command would change or run, as the user named it (REQ-MAIN-006; handoff 004e,
/// item 2): `resume`, `pause`, `cancel`, `delete`, `approve`, `reject`, `answer`, `grant`,
/// `revoke`, `rollback`, `promote`, and a fork's source (`--from-task`). The list is not kept by
/// hand: this matches **every** command, with no wildcard, so that a command that is added does
/// not compile until it says whether it acts on a task. `prune` acts on many and refuses each
/// task of them that holds such a word itself (`commands::prune`); recovery at the start is the
/// store's (`recover_all_reporting`).
fn changes_task(command: &Command) -> Option<&str> {
    match command {
        Command::Run(args) => args.from_task.as_deref(),
        Command::Task(task) => match task {
            TaskCommand::New { args, .. } => args.from_task.as_deref(),
            TaskCommand::Resume { task, .. }
            | TaskCommand::Pause { task }
            | TaskCommand::Cancel { task }
            | TaskCommand::Delete { task, .. } => Some(task),
            TaskCommand::Show { .. }
            | TaskCommand::List
            | TaskCommand::Prune { .. }
            | TaskCommand::Output { .. } => None,
        },
        Command::Approve { task, .. }
        | Command::Reject { task, .. }
        | Command::Answer { task, .. }
        | Command::Grant { task, .. }
        | Command::Revoke { task, .. }
        | Command::Rollback { task, .. }
        | Command::Promote { task, .. } => Some(task),
        Command::Events { .. }
        | Command::Metrics { .. }
        | Command::Status { .. }
        | Command::Check
        | Command::Runtime(_)
        | Command::Daemon(_)
        | Command::Model(_)
        | Command::Interpreters(_)
        | Command::Toolchain(_) => None,
    }
}

/// What a command can lead to, as far as the model backend goes (REQ-CFG-007; handoff 015a-5,
/// item 3). A step is driven only by the commands that end in `commands::finish`, which follows a
/// task that is running: `run`, `task new` (without `--hold`), `task resume`, `approve`, `reject`,
/// `answer`, and `grant` and `revoke` (a grant or a revocation that invalidates a pending approval
/// sets the task running, and a task recovered under `recovery.auto_resume` is driven by the first
/// of these commands). That is the list of starts of handoff 015a (item 3, and item 3 of 015a-5); it is
/// not kept by hand here: [`Steps::of`] matches **every** command, with no wildcard, so that a command
/// that is added does not compile until it says which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Steps {
    /// It creates no task and sets none running.
    Never,
    /// It can drive a step (it may set a task running, and it follows one that runs), without
    /// being a start of its own: it needs the backend a task runs on to exist, and contacts none
    /// (`grant`, `revoke`).
    May,
    /// It creates a task and runs no step (`task new --hold`): no backend is contacted, not even
    /// to be probed (external design §20), and the task is created whether or not its backend
    /// answers.
    Creates,
    /// It creates a task and runs it, or sets one running (`run`, `task new`, `task resume`,
    /// `approve`, `reject`, `answer`): the backend is made sure to be able to answer, and probed
    /// first (RFC 008 §4).
    Starts,
}

impl Steps {
    /// What `command` can lead to. No wildcard: see the type.
    fn of(command: &Command) -> Self {
        match command {
            Command::Run(_) => Steps::Starts,
            Command::Task(task) => match task {
                TaskCommand::New { hold: false, .. } | TaskCommand::Resume { .. } => Steps::Starts,
                TaskCommand::New { hold: true, .. } => Steps::Creates,
                TaskCommand::Show { .. }
                | TaskCommand::List
                | TaskCommand::Pause { .. }
                | TaskCommand::Cancel { .. }
                | TaskCommand::Prune { .. }
                | TaskCommand::Output { .. }
                | TaskCommand::Delete { .. } => Steps::Never,
            },
            Command::Approve { .. } | Command::Reject { .. } | Command::Answer { .. } => {
                Steps::Starts
            }
            Command::Grant { .. } | Command::Revoke { .. } => Steps::May,
            Command::Rollback { .. }
            | Command::Promote { .. }
            | Command::Events { .. }
            | Command::Metrics { .. }
            | Command::Status { .. }
            | Command::Check
            | Command::Runtime(_)
            | Command::Daemon(_)
            | Command::Model(_)
            | Command::Interpreters(_)
            | Command::Toolchain(_) => Steps::Never,
        }
    }

    /// The backend named `default` must exist: a task runs on it, and none other is picked.
    fn needs_default(self) -> bool {
        self != Steps::Never
    }

    /// The backend is probed first, and its context warning is printed. A creation that starts
    /// the task is refused when the backend does not answer; `task new --hold` contacts none.
    fn contacts_backend(self) -> bool {
        self == Steps::Starts
    }

    /// The backend must be able to answer (a fake one needs a script): said here, not at the first
    /// step (CLR-1, CLR-5).
    fn starts_a_task(self) -> bool {
        self == Steps::Starts
    }
}

/// Whether the task this command creates or starts is one the profile forbids the backend for
/// (REQ-SEC-010). An error, or a task that is not there, is "no": the command reports it itself.
fn start_is_forbidden(app: &App, command: &Command, backend: &str) -> bool {
    let named = match command {
        Command::Task(TaskCommand::Resume { task, .. })
        | Command::Approve { task, .. }
        | Command::Reject { task, .. }
        | Command::Answer { task, .. } => Some(task),
        _ => None,
    };
    let forbidden = match named {
        Some(task) => app
            .resolve_task(task)
            .and_then(|id| Ok(app.client.start_refusal(id)?.is_some())),
        None => {
            let profile = match command {
                Command::Run(args) => args.profile.as_deref(),
                Command::Task(TaskCommand::New { args, .. }) => args.profile.as_deref(),
                _ => return false,
            };
            Ok(app
                .client
                .creation_forbidden(profile, backend)
                .unwrap_or(false))
        }
    };
    forbidden.unwrap_or(false)
}

/// The backend the task of this command runs on (REQ-TASK-013): the level's, for a command that
/// creates a task, and the task's own, for a command that sets one running. `None` for any other
/// command, and when the creation is refused already or the task is not there: the command says
/// so itself.
fn target_of(app: &App, command: &Command) -> Option<mimlys::target::Target> {
    use mimlys::target::Target;
    let new = |args: &NewArgs| {
        Target::for_new(
            app,
            args.profile.as_deref(),
            args.effort
                .as_deref()
                .and_then(mimlys_core::effort::EffortLevel::parse),
            args.from_task.as_deref(),
        )
    };
    match command {
        Command::Run(args) => new(args),
        Command::Task(TaskCommand::New { args, .. }) => new(args),
        Command::Task(TaskCommand::Resume { task, .. })
        | Command::Approve { task, .. }
        | Command::Reject { task, .. }
        | Command::Answer { task, .. } => Target::for_task(app, task),
        _ => None,
    }
}

/// The commands that only read (REQ-PERS-013), and what each needs of the
/// command line. `promote --patch` and `rollback --dry-run` are read-only
/// "where they only read": `promote --patch` plans and prints and writes
/// nothing, so it is here; `rollback` has no dry run and always restores, so
/// it is not.
enum ReadCommand {
    Status,
    Storage,
    Events {
        task: Option<String>,
        follow: bool,
        since: u64,
    },
    Metrics {
        task: Option<String>,
    },
    TaskShow {
        task: String,
        arguments: bool,
    },
    TaskList,
    TaskOutput {
        task: String,
        action: Option<String>,
        attempt: Option<u32>,
        which: Option<commands::task_output::Which>,
    },
    ToolchainList,
    PromotePatch {
        task: String,
    },
}

impl ReadCommand {
    /// The read command a parsed command is, if it is one.
    fn of(command: &Command) -> Option<Self> {
        match command {
            Command::Status { storage: false } => Some(Self::Status),
            Command::Status { storage: true } => Some(Self::Storage),
            Command::Events {
                task,
                follow,
                since,
            } => Some(Self::Events {
                task: task.clone(),
                follow: *follow,
                since: *since,
            }),
            Command::Metrics { task } => Some(Self::Metrics { task: task.clone() }),
            Command::Task(TaskCommand::Show { task, arguments }) => Some(Self::TaskShow {
                task: task.clone(),
                arguments: *arguments,
            }),
            Command::Task(TaskCommand::List) => Some(Self::TaskList),
            Command::Task(TaskCommand::Output {
                task,
                action,
                attempt,
                stdout,
                stderr,
                result,
            }) => Some(Self::TaskOutput {
                task: task.clone(),
                action: action.clone(),
                attempt: *attempt,
                which: match (*stdout, *stderr, *result) {
                    (true, _, _) => Some(commands::task_output::Which::Stdout),
                    (_, true, _) => Some(commands::task_output::Which::Stderr),
                    (_, _, true) => Some(commands::task_output::Which::Result),
                    _ => None,
                },
            }),
            Command::Toolchain(ToolchainCommand::List) => Some(Self::ToolchainList),
            Command::Promote {
                task, patch: true, ..
            } => Some(Self::PromotePatch { task: task.clone() }),
            _ => None,
        }
    }
}

async fn dispatch_read(app: &ReadApp, command: ReadCommand, out: &Output) -> Result<(), AppError> {
    match command {
        ReadCommand::Status => commands::status::run(app, out).await,
        ReadCommand::Storage => commands::storage::run(app, out),
        ReadCommand::Events {
            task,
            follow,
            since,
        } => commands::events::run(app, task.as_deref(), since, follow, out).await,
        ReadCommand::Metrics { task } => commands::metrics::run(app, task.as_deref(), out),
        ReadCommand::TaskShow { task, arguments } => {
            commands::task::show(app, &task, arguments, out)
        }
        ReadCommand::TaskList => commands::task::list(app, out),
        ReadCommand::TaskOutput {
            task,
            action,
            attempt,
            which,
        } => commands::task_output::run(
            app,
            &task,
            action.as_deref(),
            attempt,
            which,
            mimlys::output::stdout_is_terminal(),
            out,
        ),
        ReadCommand::ToolchainList => commands::toolchain::list(app, out),
        ReadCommand::PromotePatch { task } => commands::promote::patch(app, &task, out),
    }
}

async fn dispatch(
    app: &App,
    command: Command,
    terminal: &mut dyn mimlys::prompt::Interaction,
    out: &Output,
) -> Result<(), AppError> {
    match command {
        Command::Run(args) => {
            commands::run::run(app, &args.into_new_task(false), terminal, out).await
        }
        Command::Task(task) => match task {
            TaskCommand::New { args, hold } => {
                commands::run::run(app, &args.into_new_task(hold), terminal, out).await
            }
            TaskCommand::Show { .. } | TaskCommand::List | TaskCommand::Output { .. } => {
                unreachable!("routed to the reader by ReadCommand::of")
            }
            TaskCommand::Resume {
                task,
                extend_steps,
                extend_deadline,
                ..
            } => {
                commands::task::resume(
                    app,
                    &task,
                    extend_steps,
                    extend_deadline.as_deref(),
                    terminal,
                    out,
                )
                .await
            }
            TaskCommand::Pause { task } => commands::task::pause(app, &task, out),
            TaskCommand::Cancel { task } => commands::task::cancel(app, &task, out),
            TaskCommand::Prune {
                state,
                older_than,
                audit,
                yes,
            } => commands::prune::run(
                app,
                &state,
                older_than.as_deref(),
                audit,
                yes,
                terminal,
                out,
            ),
            TaskCommand::Delete { task, audit, yes } => {
                commands::task::delete(app, &task, audit, yes, terminal, out)
            }
        },
        Command::Rollback {
            task,
            snapshot,
            promotion,
            yes,
            overwrite_edited,
        } => commands::rollback::run(
            app,
            &task,
            snapshot.as_deref(),
            promotion.as_deref(),
            yes,
            overwrite_edited,
            terminal,
            out,
        ),
        Command::Promote {
            task,
            yes,
            summary,
            patch: false,
        } => commands::promote::run(app, &task, yes, summary, terminal, out),
        Command::Promote { patch: true, .. } => {
            unreachable!("routed to the reader by ReadCommand::of")
        }
        Command::Approve { task, note, .. } => {
            commands::approve::run(app, &task, note, terminal, out).await
        }
        Command::Reject {
            task, note, halt, ..
        } => commands::reject::run(app, &task, note, halt, terminal, out).await,
        Command::Answer { task, text, .. } => {
            commands::answer::run(app, &task, &text, terminal, out).await
        }
        Command::Grant { task, kind } => {
            commands::grant::run(app, &task, &kind, terminal, out).await
        }
        Command::Revoke { task, grant } => {
            commands::revoke::run(app, &task, &grant, terminal, out).await
        }
        Command::Events { .. }
        | Command::Metrics { .. }
        | Command::Status { .. }
        | Command::Toolchain(ToolchainCommand::List) => {
            unreachable!("routed to the reader by ReadCommand::of")
        }
        Command::Toolchain(ToolchainCommand::Grant { path, yes }) => {
            commands::toolchain::grant(app, &path, yes, terminal, out)
        }
        Command::Toolchain(ToolchainCommand::Revoke { all: true, yes, .. }) => {
            commands::toolchain::revoke_all(app, yes, terminal, out)
        }
        Command::Toolchain(ToolchainCommand::Revoke { path, yes, .. }) => {
            // clap guarantees a path when `--all` is absent.
            commands::toolchain::revoke(app, &path.unwrap_or_default(), yes, terminal, out)
        }
        Command::Interpreters(cmd) => match cmd {
            InterpretersCommand::List => commands::interpreters::list(app, out),
            InterpretersCommand::Install { language, from } => {
                commands::interpreters::install_cmd(app, &language, from.as_deref(), out)
            }
        },
        Command::Model(ModelCommand::Probe { backend }) => {
            commands::model::probe(app, backend.as_deref(), out).await
        }
        Command::Check | Command::Runtime(_) | Command::Daemon(_) => {
            unreachable!("handled before the app opens")
        }
    }
}

/// `mimlys daemon …` (RFC 014; external design §19.5): the lifecycle of the daemon. Linux and
/// macOS; Windows has its own transport in a later release.
#[cfg(unix)]
async fn run_daemon(
    command: DaemonCommand,
    loaded: mimlys::config::Loaded,
    out: &Output,
) -> Result<(), AppError> {
    use mimlys::daemon::command as lifecycle;
    match command {
        DaemonCommand::Start {
            foreground: true,
            detached,
        } => lifecycle::serve_here(loaded, detached).await,
        // These block on the daemon's answers, so they run apart from the executor's threads.
        DaemonCommand::Start { .. } => {
            tokio::task::block_in_place(|| lifecycle::start(&loaded, out))
        }
        DaemonCommand::Stop => tokio::task::block_in_place(|| lifecycle::stop(&loaded, out)),
        DaemonCommand::Restart => tokio::task::block_in_place(|| lifecycle::restart(&loaded, out)),
        DaemonCommand::Status { log } => {
            tokio::task::block_in_place(|| lifecycle::status(&loaded, log, out))
        }
    }
}

/// The daemon is not part of this platform's release yet.
#[cfg(not(unix))]
async fn run_daemon(
    _command: DaemonCommand,
    _loaded: mimlys::config::Loaded,
    _out: &Output,
) -> Result<(), AppError> {
    Err(AppError::Io {
        detail: mimlys::messages::DAEMON_NOT_ON_THIS_PLATFORM.to_owned(),
    })
}
