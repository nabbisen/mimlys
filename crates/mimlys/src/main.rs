//! `mimlys`: the commands of external design §19.2 (REQ-CLI-001 … 005).
//! Exit codes: 0 success, 1 runtime or configuration error, 2 usage error.

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
    /// Interpreter builds for `run_script`.
    #[command(subcommand)]
    Interpreters(InterpretersCommand),
    /// The toolchain paths host tools may read (RFC 011).
    #[command(subcommand)]
    Toolchain(ToolchainCommand),
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
    // REQ-PERS-013: a command that only reads opens the store as a reader,
    // without the lock, and runs no recovery, reaping or migration. It cannot
    // call a writing method: its handle has none.
    if let Some(read) = ReadCommand::of(&cli.command) {
        let app = ReadApp::open(loaded)?;
        return dispatch_read(&app, read, out).await;
    }
    let app = App::open(loaded)?;
    if runs_steps(&cli.command) {
        app.backend_ready()?;
    }
    if needs_backend(&cli.command)
        && let Some(notice) = app.probe_backend().await?
    {
        out.line(notice);
    }
    // A context below the need is a warning on standard error, never an error:
    // the run goes on (REQ-MDL-003).
    if needs_backend(&cli.command)
        && let Some(warning) = app.context_warning()
    {
        eprintln!("{warning}");
    }
    let interactive =
        !cli.json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let stdin = std::io::stdin();
    let mut terminal = Terminal::new(stdin.lock(), std::io::stdout(), interactive);
    let result = dispatch(&app, cli.command, &mut terminal, out).await;
    let closed = app.close();
    result.and(closed)
}

/// Commands that run steps or report the backend probe the backend first (RFC 008 §4).
/// Commands that will ask the backend for a decision; without a backend
/// that can answer they stop here, not at the first step (CLR-1, CLR-5).
fn runs_steps(command: &Command) -> bool {
    matches!(
        command,
        Command::Run(_)
            | Command::Task(TaskCommand::New { hold: false, .. })
            | Command::Task(TaskCommand::Resume { .. })
            | Command::Approve { .. }
            | Command::Reject { .. }
            | Command::Answer { .. }
    )
}

fn needs_backend(command: &Command) -> bool {
    matches!(
        command,
        Command::Run(_)
            | Command::Task(TaskCommand::New { .. })
            | Command::Task(TaskCommand::Resume { .. })
            | Command::Approve { .. }
            | Command::Reject { .. }
            | Command::Answer { .. }
    )
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
    },
    TaskList,
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
            Command::Task(TaskCommand::Show { task }) => {
                Some(Self::TaskShow { task: task.clone() })
            }
            Command::Task(TaskCommand::List) => Some(Self::TaskList),
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
        ReadCommand::TaskShow { task } => commands::task::show(app, &task, out),
        ReadCommand::TaskList => commands::task::list(app, out),
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
            TaskCommand::Show { .. } | TaskCommand::List => {
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
        } => commands::rollback::run(app, &task, snapshot.as_deref(), promotion.as_deref(), out),
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
        Command::Check => unreachable!("handled before the app opens"),
    }
}
