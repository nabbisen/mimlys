//! `mimlys`: the commands of external design §19.2 (REQ-CLI-001 … 005).
//! Exit codes: 0 success, 1 runtime or configuration error, 2 usage error.

use clap::{Args, Parser, Subcommand};
use mimlys::app::{App, AppError, ReadApp};
use mimlys::commands::run::NewTask;
use mimlys::config::Config;
use mimlys::output::Output;
use mimlys::prompt::{StdinLines, Terminal};
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
    /// Approve the pending action; it runs at once.
    Approve {
        /// Task id or a unique prefix.
        task: String,
    },
    /// Reject the pending action.
    Reject {
        /// Task id or a unique prefix.
        task: String,
        /// A note for the model.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        /// Halt the task instead of continuing.
        #[arg(long)]
        halt: bool,
    },
    /// Answer the pending question.
    Answer {
        /// Task id or a unique prefix.
        task: String,
        /// The answer.
        text: String,
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
        eprintln!("mimlys: cannot mark the process non-dumpable: {e}");
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
    match runtime.block_on(run(cli, &out)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            out.error(&e);
            ExitCode::from(1)
        }
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
    if let Some(warning) = logging::init(&loaded.config.logging, &loaded.secrets) {
        eprintln!("{warning}");
    }
    if let Command::Check = &cli.command {
        return commands::check::run(&loaded, out);
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
        return dispatch_read(&app, read, out).await;
    }
    // Every writer is refused while another process holds the store, naming it (LIM-O3): the
    // refusal is made where the store is opened (`App::open`), by one function.
    let app = App::open(loaded)?;
    // A task runs on the backend named `default`, and no command picks another when there is
    // none (REQ-CFG-007; RFC 015, Amendment 1): this is said before anything is created or run.
    // Every command that can lead to a step says it, `grant` and `revoke` included (`Steps`).
    let steps = Steps::of(&cli.command);
    if steps.needs_default() {
        app.loaded.config.default_backend()?;
    }
    if steps.starts_a_task() {
        app.backend_ready()?;
    }
    // REQ-SEC-010: a backend that the start of this command's task is refused is not contacted,
    // not even to be probed; the command goes on and says why (the creation is refused, or the
    // task halts `policy`).
    let forbidden = start_is_forbidden(&app, &cli.command);
    if steps.contacts_backend()
        && !forbidden
        && let Some(notice) = app.probe_backend().await?
    {
        out.line(notice);
    }
    // A context below the need is a warning on standard error, never an error:
    // the run goes on (REQ-MDL-003).
    if steps.contacts_backend()
        && let Some(warning) = app.context_warning()
    {
        eprintln!("{warning}");
    }
    let interactive =
        !cli.json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    // Standard input is read on a thread of its own, so that a stop signal at a
    // prompt ends the wait (REQ-CLI-005).
    let mut terminal = Terminal::with_lines(StdinLines::new(), std::io::stdout(), interactive);
    let result = dispatch(&app, cli.command, &mut terminal, out).await;
    let closed = app.close();
    result.and(closed)
}

/// What a command can lead to, as far as the model backend goes (REQ-CFG-007; handoff 015a-5,
/// item 3). A step is driven only by the commands that end in `commands::finish`, which follows a
/// task that is running: `run`, `task new` (without `--hold`), `task resume`, `approve`, `reject`,
/// `answer`, and `grant` and `revoke` (a grant or a revocation that invalidates a pending approval
/// sets the task running, and a task recovered under `recovery.auto_resume` is driven by the first
/// of these commands). That is the list of starts of the request 015a-1 (section 5); it is not
/// kept by hand here: [`Steps::of`] matches **every** command, with no wildcard, so that a command
/// that is added does not compile until it says which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Steps {
    /// It creates no task and sets none running.
    Never,
    /// It can drive a step (it may set a task running, and it follows one that runs), without
    /// being a start of its own: it needs the backend a task runs on to exist, and contacts none
    /// (`grant`, `revoke`).
    May,
    /// It creates a task and runs no step (`task new --hold`): the backend is probed, as the
    /// creation checks it.
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
            | Command::Model(_)
            | Command::Interpreters(_)
            | Command::Toolchain(_) => Steps::Never,
        }
    }

    /// The backend named `default` must exist: a task runs on it, and none other is picked.
    fn needs_default(self) -> bool {
        self != Steps::Never
    }

    /// The backend is probed first, and its context warning is printed.
    fn contacts_backend(self) -> bool {
        matches!(self, Steps::Creates | Steps::Starts)
    }

    /// The backend must be able to answer (a fake one needs a script): said here, not at the first
    /// step (CLR-1, CLR-5).
    fn starts_a_task(self) -> bool {
        self == Steps::Starts
    }
}

/// Whether the task this command creates or starts is one the profile forbids the backend for
/// (REQ-SEC-010). An error, or a task that is not there, is "no": the command reports it itself.
fn start_is_forbidden(app: &App, command: &Command) -> bool {
    let named = match command {
        Command::Task(TaskCommand::Resume { task, .. })
        | Command::Approve { task }
        | Command::Reject { task, .. }
        | Command::Answer { task, .. } => Some(task),
        _ => None,
    };
    let forbidden = match named {
        Some(task) => app
            .resolve_task(task)
            .and_then(|id| Ok(app.runtime.get_task(id)?))
            .and_then(|view| Ok(app.runtime.start_refusal(&view.task)?.is_some())),
        None => {
            let profile = match command {
                Command::Run(args) => args.profile.as_deref(),
                Command::Task(TaskCommand::New { args, .. }) => args.profile.as_deref(),
                _ => return false,
            };
            Ok(app.runtime.creation_forbidden(profile).unwrap_or(false))
        }
    };
    forbidden.unwrap_or(false)
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
            std::io::stdout().is_terminal(),
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
        Command::Approve { task } => commands::approve::run(app, &task, terminal, out).await,
        Command::Reject { task, note, halt } => {
            commands::reject::run(app, &task, note, halt, terminal, out).await
        }
        Command::Answer { task, text } => {
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
        Command::Check | Command::Runtime(_) => unreachable!("handled before the app opens"),
    }
}
