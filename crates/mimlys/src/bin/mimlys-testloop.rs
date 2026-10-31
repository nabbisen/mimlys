//! `mimlys-testloop`: the M1 test harness (REQ-TEST-002, REQ-TEST-007,
//! REQ-TEST-016). Runs one task with the fake backend from a script file
//! and the built-in tools over a real scope copy, answers approvals and
//! questions from the command line instead of a terminal, honours the
//! crash-injection variable of the persistence crate, and prints a JSON
//! report of the task's records. Used by the M1 demonstration tests.

use clap::Parser;
use mimlys::app::{App, AppError};
use mimlys::commands::run::{NewTask, create};
use mimlys::config::{Config, Loaded};
use mimlys::follow;
use mimlys::output::Output;
use mimlys::prompt::{Decision, Scripted};
use mimlys_core::task::TaskState;
use mimlys_model::fake::FakeBackend;
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "mimlys-testloop",
    about = "mimlys test harness: one task over the fakes"
)]
struct Cli {
    /// Script file of decisions (the fake backend's format).
    #[arg(long)]
    script: PathBuf,
    /// Data directory.
    #[arg(long)]
    data_dir: PathBuf,
    /// Project scope.
    #[arg(long)]
    scope: PathBuf,
    /// Goal.
    #[arg(long, default_value = "demonstrate the loop")]
    goal: String,
    /// Profile.
    #[arg(long, default_value = "workspace")]
    profile: String,
    /// Tools needing approval in the profile; repeatable.
    #[arg(long = "approval-tool")]
    approval_tools: Vec<String>,
    /// Completion check JSON.
    #[arg(long)]
    completion_check: Option<String>,
    /// What the fake user decides at every approval: approve, reject, halt.
    #[arg(long, default_value = "approve")]
    decide: String,
    /// The fake user's answer to every question.
    #[arg(long, default_value = "yes")]
    answer: String,
    /// Resume this task (or `latest`) instead of creating one; creates one
    /// when `latest` finds no task (after a crash before creation).
    #[arg(long)]
    resume: Option<String>,
    /// Step budget.
    #[arg(long)]
    steps: Option<u32>,
    /// `[sandbox] deadline_ms`.
    #[arg(long)]
    sandbox_deadline_ms: Option<i64>,
    /// `[sandbox] fuel`.
    #[arg(long)]
    sandbox_fuel: Option<u64>,
    /// `[sandbox] memory_mib`.
    #[arg(long)]
    sandbox_memory_mib: Option<u32>,
    /// `[sandbox] output_bytes`.
    #[arg(long)]
    sandbox_output_bytes: Option<u64>,
    /// A JavaScript interpreter module to install into the data directory before the run.
    #[arg(long)]
    interpreter_module: Option<PathBuf>,
    /// Write the runtime log to standard error at this level (`info`, `debug`, …).
    #[arg(long)]
    log: Option<String>,
}

#[derive(Serialize)]
struct Report {
    task_id: String,
    state: TaskState,
    task: mimlys_core::task::Task,
    steps: Vec<mimlys_core::step::Step>,
    actions: Vec<mimlys_core::action::Action>,
    executions: Vec<mimlys_core::action::Execution>,
    observations: Vec<mimlys_core::observation::Observation>,
    approvals: Vec<ApprovalReport>,
    events: Vec<EventReport>,
    feedback: Vec<Vec<String>>,
    requests: usize,
    env_names: Vec<String>,
    recovered: Vec<String>,
}

#[derive(Serialize)]
struct ApprovalReport {
    status: mimlys_core::approval::ApprovalStatus,
    text: String,
}

#[derive(Serialize)]
struct EventReport {
    position: u64,
    name: String,
    actor: String,
    action_id: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(level) = &cli.log {
        let _ = mimlys::logging::init(
            &mimlys::config::LoggingSection {
                level: level.clone(),
                debug_prompts: false,
            },
            &[],
        );
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio");
    match runtime.block_on(run(cli)) {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("report serializes")
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", e.to_json());
            ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Result<Report, AppError> {
    let mut config = Config::default();
    config.runtime.data_dir = Some(cli.data_dir.clone());
    config.policy.profiles.insert(
        cli.profile.clone(),
        mimlys::config::ProfileSection {
            approval_tools: if cli.approval_tools.is_empty() {
                None
            } else {
                Some(cli.approval_tools.clone())
            },
            steps: cli.steps,
            ..Default::default()
        },
    );
    if let Some(v) = cli.sandbox_deadline_ms {
        config.sandbox.deadline_ms = v;
    }
    if let Some(v) = cli.sandbox_fuel {
        config.sandbox.fuel = v;
    }
    if let Some(v) = cli.sandbox_memory_mib {
        config.sandbox.memory_mib = v;
    }
    if let Some(v) = cli.sandbox_output_bytes {
        config.sandbox.output_bytes = v;
    }
    config.validate()?;
    if let Some(module) = &cli.interpreter_module {
        let bytes = std::fs::read(module).map_err(|e| AppError::Io {
            detail: format!("interpreter module: {e}"),
        })?;
        let pinned = mimlys_exec::interpreters::pinned("javascript").expect("javascript is pinned");
        mimlys_exec::interpreters::install(&cli.data_dir, "javascript", pinned.sha256, &bytes)
            .map_err(|e| AppError::Io {
                detail: e.to_string(),
            })?;
    }
    let backend = Arc::new(
        FakeBackend::from_script(&cli.script).map_err(|e| AppError::Io {
            detail: format!("script: {e}"),
        })?,
    );
    let app = App::open_with(
        Loaded {
            config,
            path: None,
            secrets: vec![],
        },
        Some(backend.clone()),
    )?;
    let existing = match cli.resume.as_deref() {
        Some("latest") => app.runtime.list_tasks()?.first().map(|t| t.task_id),
        Some(id) => Some(app.resolve_task(id)?),
        None => None,
    };
    let task_id = match existing {
        Some(id) => {
            let state = app.runtime.get_task(id)?.task.state;
            if matches!(
                state,
                TaskState::Created | TaskState::Paused | TaskState::Halted { .. }
            ) {
                app.runtime.resume_task(&app.session, id, None)?;
            }
            id
        }
        None => {
            create(
                &app,
                &NewTask {
                    goal: cli.goal.clone(),
                    scope: cli.scope.clone(),
                    profile: Some(cli.profile.clone()),
                    grants: vec![],
                    steps: None,
                    deadline: None,
                    completion_check: cli.completion_check.clone(),
                    hold: false,
                    from_task: None,
                    from_snapshot: None,
                },
            )?
            .task_id
        }
    };
    let decision = match cli.decide.as_str() {
        "approve" => Decision::Approve,
        "reject" => Decision::Reject {
            note: Some("the harness rejected it".into()),
            halt: false,
        },
        "halt" => Decision::Reject {
            note: None,
            halt: true,
        },
        other => {
            return Err(AppError::Io {
                detail: format!("--decide {other}: use approve, reject or halt"),
            });
        }
    };
    let mut scripted = Scripted {
        decision,
        answer: cli.answer.clone(),
    };
    let out = Output::new(true);
    let view = follow::drive(&app, task_id, &mut scripted, &out).await?;
    let persistence = app.runtime.persistence();
    let actions = persistence.load_actions(task_id)?;
    let mut executions = Vec::new();
    for a in &actions {
        executions.extend(persistence.load_executions(a.action_id)?);
    }
    let feedback = backend
        .requests()
        .iter()
        .map(|r| {
            r.messages
                .iter()
                .filter(|m| m.content.starts_with("<<feedback"))
                .map(|m| m.content.clone())
                .collect()
        })
        .collect();
    let events = app
        .runtime
        .get_events(Some(task_id), 0, 10_000)?
        .iter()
        .map(|e| EventReport {
            position: e.position,
            name: e.record.event.name().to_owned(),
            actor: serde_json::to_value(&e.record.actor)
                .ok()
                .and_then(|v| v.get("actor").and_then(|a| a.as_str()).map(str::to_owned))
                .unwrap_or_default(),
            action_id: e.record.action_id.map(|id| id.to_string()),
        })
        .collect();
    let report = Report {
        task_id: task_id.to_string(),
        state: view.task.state.clone(),
        steps: persistence.load_steps(task_id)?,
        executions,
        observations: persistence.load_observations(task_id)?,
        approvals: persistence
            .load_approvals(task_id)?
            .iter()
            .map(|a| ApprovalReport {
                status: a.status,
                text: a.display_text(),
            })
            .collect(),
        events,
        feedback,
        requests: backend.requests().len(),
        env_names: std::env::vars_os()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect(),
        recovered: app
            .recovered
            .iter()
            .map(|r| {
                format!(
                    "{}:{}",
                    serde_json::to_string(&r.disposition).unwrap_or_default(),
                    r.state
                )
            })
            .collect(),
        actions,
        task: view.task,
    };
    app.close()?;
    Ok(report)
}
