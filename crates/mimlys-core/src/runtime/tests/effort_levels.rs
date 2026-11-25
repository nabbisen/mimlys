//! Effort levels in the runtime (handoff 015c; REQ-TASK-013, REQ-CFG-007, REQ-PERS-015,
//! REQ-OBS-010; RFC 015 with Amendment 1; external design §19.4, §20): the envelope a task is
//! created with and its precedence, what is refused at creation and leaves nothing, a task on a
//! backend of its own that is found by its name at every start, `recent_steps` read from the task,
//! and the record and its readers. Every test decides by what the backends received and by the
//! state of the record, never by the order of calls.

use super::*;
use crate::effort::{ChosenBy, Effort, EffortLevel, Envelope, LevelKeys, Levels};
use crate::spi::{BackendDirectory, ModelBackend, ResolvedBackend, UnknownBackend};
use crate::task::HaltDetail;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The configured backends of a test: each by its name, the scripted backend behind it. A name
/// that is removed is a backend that the file no longer has.
struct Directory {
    backends: Mutex<BTreeMap<String, Arc<ScriptedBackend>>>,
    /// The `context_chars` the file gives a backend; the register's default where none is set.
    contexts: Mutex<BTreeMap<String, usize>>,
    /// The configured backends that cannot be made, and why (a key by reference that the
    /// environment does not hold).
    unusable: Mutex<BTreeMap<String, String>>,
}

impl Directory {
    fn new(entries: &[(&str, &Arc<ScriptedBackend>)]) -> Arc<Self> {
        Arc::new(Self {
            backends: Mutex::new(
                entries
                    .iter()
                    .map(|(name, b)| ((*name).to_owned(), Arc::clone(b)))
                    .collect(),
            ),
            contexts: Mutex::new(BTreeMap::new()),
            unusable: Mutex::new(BTreeMap::new()),
        })
    }

    /// The environment lost what `name` needs (`Some(reason)`), or has it again (`None`): the
    /// backend is configured and cannot be made.
    fn set_unusable(&self, name: &str, reason: Option<&str>) {
        let mut unusable = self.unusable.lock().unwrap();
        match reason {
            Some(reason) => unusable.insert(name.to_owned(), reason.to_owned()),
            None => unusable.remove(name),
        };
    }

    /// A user edited `backends.<name>.context_chars`: the next process reads it.
    fn set_context(&self, name: &str, chars: usize) {
        self.contexts.lock().unwrap().insert(name.to_owned(), chars);
    }

    /// The file no longer has `name`.
    fn remove(&self, name: &str) {
        self.backends.lock().unwrap().remove(name);
    }

    /// A new process made another adapter for `name`.
    fn replace(&self, name: &str, backend: &Arc<ScriptedBackend>) {
        self.backends
            .lock()
            .unwrap()
            .insert(name.to_owned(), Arc::clone(backend));
    }
}

impl BackendDirectory for Directory {
    fn resolve(&self, name: &str) -> Result<ResolvedBackend, UnknownBackend> {
        if let Some(reason) = self.unusable.lock().unwrap().get(name) {
            return Err(UnknownBackend {
                name: name.to_owned(),
                configured: self.names(),
                unusable: Some(reason.clone()),
            });
        }
        let found = self.backends.lock().unwrap().get(name).cloned();
        match found {
            Some(backend) => Ok(ResolvedBackend {
                backend: backend as Arc<dyn ModelBackend>,
                gate: None,
                context_chars: self
                    .contexts
                    .lock()
                    .unwrap()
                    .get(name)
                    .copied()
                    .unwrap_or(crate::limits::LIM_C1_CONTEXT_CHARS),
            }),
            None => Err(UnknownBackend {
                name: name.to_owned(),
                configured: self.names(),
                unusable: None,
            }),
        }
    }

    fn names(&self) -> Vec<String> {
        self.backends.lock().unwrap().keys().cloned().collect()
    }
}

/// `deep` names the backend `deep`.
fn deep_on(backend: &str) -> Levels {
    Levels {
        deep: LevelKeys {
            backend: Some(backend.to_owned()),
            ..LevelKeys::default()
        },
        ..Levels::shipped()
    }
}

/// The harness, with the levels' keys and the configured backends of a file.
fn with(mut h: Harness, directory: &Arc<Directory>, levels: Levels) -> Harness {
    h.runtime = h
        .runtime
        .with_backends(directory.clone())
        .with_effort_levels(levels);
    h
}

fn effort_of(h: &Harness, id: TaskId) -> Effort {
    h.runtime
        .get_task(id)
        .unwrap()
        .task
        .effort
        .expect("a task made now records its level")
}

fn standard_envelope(steps: u32, deadline_ms: i64) -> Envelope {
    Envelope {
        backend: "default".into(),
        steps,
        deadline_ms,
        context_chars: None,
        recent_steps: 6,
    }
}

/// A scope of its own: one task is active on a scope at a time.
fn scope_of(n: u32) -> ScopeSpec {
    ScopeSpec {
        root: format!("/home/u/project{n}"),
        include: vec![],
    }
}

/// What a creation was refused with must leave nothing: no task, no event, no workspace.
fn nothing_made(h: &Harness, what: &str) {
    assert!(h.persistence.list_tasks().unwrap().is_empty(), "{what}");
    assert!(h.persistence.events().is_empty(), "{what}: an event");
    assert_eq!(h.workspaces.materialized(), vec![], "{what}: a workspace");
}

/// REQ-TASK-013: a task's recorded envelope is what it runs with, every key resolved, by one
/// precedence: a flag over the level, the level over the profile, the profile over the register's
/// default, a key of the file over the one the level ships with. Each case is one task whose
/// record is checked whole, with the deadline and the budgets the task really has.
#[test]
fn req_task_013_the_envelope_follows_the_precedence_flag_level_profile_default() {
    let mut policy = Policy::shipped();
    policy.profiles.get_mut("workspace").unwrap().budgets.steps = 30;
    let big = Arc::new(ScriptedBackend::with_decisions(vec![]));
    let default = Arc::new(ScriptedBackend::with_decisions(vec![]));
    let directory = Directory::new(&[("default", &default), ("deep", &big)]);

    let flags = |steps: u32| Budgets {
        steps,
        ..Budgets::default()
    };
    type Edit = Box<dyn Fn(&mut Levels)>;
    // (what the case is, the file's keys, the flags, the level asked for, what is recorded)
    type Case = (
        &'static str,
        Edit,
        Option<Budgets>,
        Option<i64>,
        Option<EffortLevel>,
        Effort,
    );
    let cases: Vec<Case> = vec![
        (
            "the profile alone: no flag, no key",
            Box::new(|_| {}),
            None,
            None,
            None,
            Effort {
                level: EffortLevel::Standard,
                chosen_by: ChosenBy::Default,
                envelope: standard_envelope(30, crate::limits::LIM_T5_DEADLINE_MS),
            },
        ),
        (
            "the shipped keys of quick over the profile and the register",
            Box::new(|_| {}),
            None,
            None,
            Some(EffortLevel::Quick),
            Effort {
                level: EffortLevel::Quick,
                chosen_by: ChosenBy::User,
                envelope: standard_envelope(22, 600_000),
            },
        ),
        (
            "a key of the file replaces the shipped one, and only that one",
            Box::new(|levels: &mut Levels| levels.quick.steps = Some(15)),
            None,
            None,
            Some(EffortLevel::Quick),
            Effort {
                level: EffortLevel::Quick,
                chosen_by: ChosenBy::User,
                envelope: standard_envelope(15, 600_000),
            },
        ),
        (
            "the level over the profile, for standard too",
            Box::new(|levels: &mut Levels| {
                levels.standard.steps = Some(12);
                levels.standard.recent_steps = Some(4);
            }),
            None,
            None,
            None,
            Effort {
                level: EffortLevel::Standard,
                chosen_by: ChosenBy::Default,
                envelope: Envelope {
                    recent_steps: 4,
                    ..standard_envelope(12, crate::limits::LIM_T5_DEADLINE_MS)
                },
            },
        ),
        (
            "the flags over the level",
            Box::new(|levels: &mut Levels| levels.quick.steps = Some(15)),
            Some(flags(7)),
            Some(120_000),
            Some(EffortLevel::Quick),
            Effort {
                level: EffortLevel::Quick,
                chosen_by: ChosenBy::User,
                envelope: standard_envelope(7, 120_000),
            },
        ),
        (
            "a level that names a backend, and its other keys",
            Box::new(|levels: &mut Levels| {
                levels.deep = LevelKeys {
                    backend: Some("deep".into()),
                    steps: Some(60),
                    deadline_ms: Some(3_600_000),
                    context_chars: Some(20_000),
                    recent_steps: Some(3),
                };
            }),
            None,
            None,
            Some(EffortLevel::Deep),
            Effort {
                level: EffortLevel::Deep,
                chosen_by: ChosenBy::User,
                envelope: Envelope {
                    backend: "deep".into(),
                    steps: 60,
                    deadline_ms: 3_600_000,
                    context_chars: Some(20_000),
                    recent_steps: 3,
                },
            },
        ),
    ];
    for (what, edit, budgets, deadline_ms, level, expected) in cases {
        let mut levels = Levels::shipped();
        edit(&mut levels);
        let h = with(
            Harness::with_policy(vec![], policy.clone()),
            &directory,
            levels,
        );
        let id = h.create_with(|r| {
            r.hold = true;
            r.budgets = budgets;
            r.deadline_ms = deadline_ms;
            r.effort = level;
        });
        assert_eq!(effort_of(&h, id), expected, "{what}");
        // What the task runs with is what it recorded.
        let task = h.runtime.get_task(id).unwrap().task;
        assert_eq!(task.budgets.steps, expected.envelope.steps, "{what}");
        assert_eq!(
            task.created_at.millis_until(task.deadline),
            expected.envelope.deadline_ms,
            "{what}"
        );
        assert_eq!(task.recent_steps(), expected.envelope.recent_steps as usize);
        // The same record is in the creation event.
        let created = h
            .persistence
            .events()
            .into_iter()
            .find(|e| e.event.name() == "task.created")
            .unwrap();
        match created.event {
            crate::event::Event::TaskCreated { effort, .. } => {
                assert_eq!(effort, Some(expected), "{what}: the event")
            }
            other => panic!("{other:?}"),
        }
    }
}

/// REQ-TASK-013, LIM-C1 (external design §19.4, review of 015c): the envelope records
/// `context_chars` only when the level sets it, a record that holds a value reads as the level's
/// own, and the budget at a step is the level's value bounded by the backend's as it is then, or
/// the backend's alone.
#[test]
fn req_task_013_the_envelope_records_context_chars_only_when_the_level_sets_it() {
    let default = Arc::new(ScriptedBackend::with_decisions(vec![]));
    let deep = Arc::new(ScriptedBackend::with_decisions(vec![]));
    let directory = Directory::new(&[("default", &default), ("deep", &deep)]);
    let mut levels = deep_on("deep");
    levels.standard.context_chars = Some(12_000);
    let h = with(
        Harness::with_context(vec![], Policy::shipped(), vec![], 16_000),
        &directory,
        levels,
    );
    // `standard` sets a value: it is recorded. `deep` sets none: nothing is recorded, though its
    // backend has one.
    let standard = h.create_with(|r| r.hold = true);
    let on_deep = h.create_with(|r| {
        r.hold = true;
        r.scope = scope_of(2);
        r.effort = Some(EffortLevel::Deep);
    });
    assert_eq!(effort_of(&h, standard).envelope.context_chars, Some(12_000));
    assert_eq!(effort_of(&h, on_deep).envelope.context_chars, None);
    let record = serde_json::to_value(effort_of(&h, on_deep)).unwrap();
    assert!(
        record["envelope"].get("context_chars").is_none(),
        "{record}"
    );
    // A record of this release's earlier builds, which holds the backend's value resolved at the
    // creation, reads as the level's own: bounded by the backend's now, and no more.
    let held: Effort = serde_json::from_value(serde_json::json!({
        "level": "deep", "chosen_by": "user",
        "envelope": {"backend": "deep", "steps": 40, "deadline_ms": 1_800_000,
                     "context_chars": 32_000, "recent_steps": 6}
    }))
    .unwrap();
    assert_eq!(held.envelope.context_chars, Some(32_000));
    assert_eq!(
        crate::effort::context_budget(held.envelope.context_chars, 8_000).chars,
        8_000
    );
}

/// REQ-TASK-013, LIM-C1 (review of 015c, item 1): lowering a backend's `context_chars` between
/// two processes lowers the budget of a task already made, the remedy `status` gives for an engine
/// whose window is too small; a level's own value stays, bounded by the backend's. Each case runs
/// the scenario of `req_ctx_003`: what the budget did is the steps it had to compress.
#[test]
fn req_task_013_lowering_the_backends_context_chars_lowers_a_running_tasks_budget() {
    const N: usize = 14;
    const LOWERED_AT: usize = 7;
    // The sizes the tight budget is derived from, as `recent_steps`' test does.
    let generous = Harness::new(context_scenario(N));
    script_context_outcomes(&generous, N);
    let id = generous.create_with(|r| r.profile = Some("read_only".into()));
    for _ in 0..N {
        generous.step(id);
    }
    let (last, first) = (request_chars(&generous, N - 1), request_chars(&generous, 0));
    let tight = first + (last - first) / 3;
    let wide = crate::limits::LIM_C1_CONTEXT_CHARS;

    // The steps each request compressed. The task is created on a backend whose `context_chars` is
    // `at_creation`; the file is edited to `lowered` before step `LOWERED_AT`, as a user does
    // between two commands; the level sets `level` or nothing.
    let compressed = |level: Option<usize>, at_creation: usize, lowered: Option<usize>| {
        let default = Arc::new(ScriptedBackend::with_decisions(vec![]));
        let deep = Arc::new(ScriptedBackend::with_decisions(context_scenario(N)));
        let directory = Directory::new(&[("default", &default), ("deep", &deep)]);
        directory.set_context("deep", at_creation);
        let mut levels = deep_on("deep");
        levels.deep.context_chars = level.map(|chars| chars as u32);
        let h = with(
            Harness::with_policy(vec![], Policy::shipped()),
            &directory,
            levels,
        );
        script_context_outcomes(&h, N);
        let id = h.create_with(|r| {
            r.profile = Some("read_only".into());
            r.effort = Some(EffortLevel::Deep);
        });
        assert_eq!(
            effort_of(&h, id).envelope.context_chars,
            level.map(|chars| chars as u32),
            "the envelope holds the level's value and nothing else"
        );
        for k in 0..N {
            if k == LOWERED_AT
                && let Some(chars) = lowered
            {
                directory.set_context("deep", chars);
            }
            assert_eq!(h.step(id).outcome, StepOutcome::Continued);
        }
        (0..N)
            .map(|k| measures(&h, id, k).context.compressed_steps)
            .collect::<Vec<u32>>()
    };
    let before = |c: &[u32]| c[..LOWERED_AT].iter().sum::<u32>();
    let after = |c: &[u32]| c[LOWERED_AT..].iter().sum::<u32>();

    // The control: nothing lowered, nothing compressed.
    let kept = compressed(None, wide, None);
    assert_eq!((before(&kept), after(&kept)), (0, 0), "{kept:?}");
    // The level sets none: the backend's budget as it is at each step. Lowered in the middle, the
    // steps before are as they were and the ones after compress.
    let followed = compressed(None, wide, Some(tight));
    assert_eq!(before(&followed), 0, "{followed:?}");
    assert!(
        after(&followed) > 0,
        "the lowered budget reached the task: {followed:?}"
    );
    // The level's own value stays, and is bounded by the backend's: a value as wide as the
    // backend's was is bounded when the backend is lowered...
    let bounded = compressed(Some(wide), wide, Some(tight));
    assert_eq!(before(&bounded), 0, "{bounded:?}");
    assert!(
        after(&bounded) > 0,
        "bounded by the backend's now: {bounded:?}"
    );
    // ...and a level's tighter value is the budget while the backend's is wider, whatever the
    // file says of its backend: the same steps compress as when the backend has that value.
    let own = compressed(Some(tight), wide, None);
    assert!(
        own.iter().sum::<u32>() > 0,
        "the level's own value is the budget: {own:?}"
    );
    assert_eq!(
        own,
        compressed(None, tight, None),
        "the same budget, from either source"
    );
}

/// REQ-TASK-013: a level with no key, shipped or configured, is refused at creation, naming the
/// section that configures it; and no task, no event and no workspace is left (external design
/// §20). `standard` is never refused for it.
#[test]
fn req_task_013_a_level_without_a_key_is_refused_and_nothing_is_made() {
    let h = Harness::new(vec![]);
    let error = h
        .runtime
        .create_task(
            &h.session,
            CreateTask {
                effort: Some(EffortLevel::Deep),
                ..create_request()
            },
        )
        .unwrap_err();
    assert!(
        matches!(&error, RuntimeError::InvalidRequest { detail } if detail.contains("[effort.deep]") && detail.contains("backend")),
        "{error:?}"
    );
    nothing_made(&h, "deep without a key");
    // `standard` has no key either, and is always chosen.
    let id = h.create_with(|r| r.effort = Some(EffortLevel::Standard));
    assert_eq!(effort_of(&h, id).level, EffortLevel::Standard);
    assert_eq!(effort_of(&h, id).chosen_by, ChosenBy::User);
}

/// REQ-TASK-013: `effort_max` bounds the levels. A level above the bound is refused naming what
/// sets it; **without the flag** the level is `standard`, or the bound when it is lower; the
/// ceiling bounds the profiles.
#[test]
fn req_task_013_effort_max_refuses_above_and_lowers_the_default() {
    let default = Arc::new(ScriptedBackend::with_decisions(vec![]));
    let deep = Arc::new(ScriptedBackend::with_decisions(vec![]));
    let directory = Directory::new(&[("default", &default), ("deep", &deep)]);

    // The ceiling's bound: `deep` is refused, naming it; `standard` goes.
    let mut policy = Policy::shipped();
    policy.ceiling.effort_max = Some(EffortLevel::Standard);
    let h = with(
        Harness::with_policy(vec![], policy),
        &directory,
        deep_on("deep"),
    );
    let error = h
        .runtime
        .create_task(
            &h.session,
            CreateTask {
                effort: Some(EffortLevel::Deep),
                ..create_request()
            },
        )
        .unwrap_err();
    assert!(
        matches!(&error, RuntimeError::Denied { detail } if detail.contains("[policy.ceiling] effort_max") && detail.contains("`standard`")),
        "{error:?}"
    );
    nothing_made(&h, "above the ceiling's effort_max");
    let id = h.create_with(|r| r.hold = true);
    assert_eq!(effort_of(&h, id).level, EffortLevel::Standard);

    // The profile's bound, lower than the ceiling's: without the flag, the level is the bound,
    // chosen by default; `standard` with the flag is refused.
    let mut policy = Policy::shipped();
    policy.ceiling.effort_max = Some(EffortLevel::Deep);
    policy.profiles.get_mut("workspace").unwrap().effort_max = Some(EffortLevel::Quick);
    let h = with(
        Harness::with_policy(vec![], policy),
        &directory,
        deep_on("deep"),
    );
    let id = h.create_with(|r| r.hold = true);
    let recorded = effort_of(&h, id);
    assert_eq!(recorded.level, EffortLevel::Quick, "the bound is lower");
    assert_eq!(recorded.chosen_by, ChosenBy::Default);
    assert_eq!(recorded.envelope.steps, 22, "quick's shipped keys");
    let error = h
        .runtime
        .create_task(
            &h.session,
            CreateTask {
                effort: Some(EffortLevel::Standard),
                ..create_request()
            },
        )
        .unwrap_err();
    assert!(
        matches!(&error, RuntimeError::Denied { detail } if detail.contains("[policy.profiles.workspace] effort_max")),
        "{error:?}"
    );
}

/// REQ-TASK-013: a level whose backend is not configured is refused at creation, naming the
/// configured ones, and nothing is made.
#[test]
fn req_task_013_a_level_on_a_backend_that_is_not_configured_is_refused() {
    let default = Arc::new(ScriptedBackend::with_decisions(vec![]));
    let directory = Directory::new(&[("default", &default)]);
    let h = with(Harness::new(vec![]), &directory, deep_on("deep"));
    let error = h
        .runtime
        .create_task(
            &h.session,
            CreateTask {
                effort: Some(EffortLevel::Deep),
                ..create_request()
            },
        )
        .unwrap_err();
    assert!(
        matches!(&error, RuntimeError::InvalidRequest { detail } if detail.contains("`deep`") && detail.contains("configured: `default`")),
        "{error:?}"
    );
    nothing_made(&h, "a backend that is not configured");
}

/// REQ-TASK-013, REQ-PERS-015: a task on a second backend sends its requests to that backend and
/// none to `default`'s; each step records the backend, the model and the transport it was decided
/// with; the task's record holds the backend it was made on.
#[test]
fn req_pers_015_a_task_on_a_second_backend_sends_there_and_records_it_per_step() {
    let default =
        Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "d")]).with_model("small"));
    let deep = Arc::new(
        ScriptedBackend::with_decisions(vec![act("read_file", "a"), act("read_file", "b")])
            .with_model("large")
            .tool_calls(),
    );
    let directory = Directory::new(&[("default", &default), ("deep", &deep)]);
    let h = with(
        Harness::with_backend(
            Arc::clone(&default),
            Policy::shipped(),
            vec![],
            crate::limits::LIM_C1_CONTEXT_CHARS,
        ),
        &directory,
        deep_on("deep"),
    );
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|r| r.effort = Some(EffortLevel::Deep));
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(deep.requests().len(), 2, "the deep backend's requests");
    assert!(
        default.requests().is_empty(),
        "none to the backend named `default`"
    );
    let task = h.runtime.get_task(id).unwrap().task;
    assert_eq!(
        task.versions.backend.as_ref().unwrap().model.as_deref(),
        Some("large"),
        "the record holds the backend the task was made on"
    );
    let steps = h.persistence.load_steps(id).unwrap();
    assert_eq!(steps.len(), 2);
    for step in &steps {
        assert_eq!(step.backend.as_deref(), Some("deep"));
        assert_eq!(step.model.as_deref(), Some("large"));
        assert_eq!(
            step.transport,
            Some(crate::native::DecisionTransport::ToolCalls)
        );
    }

    // A task on `default` records `default` and its own model and transport (one task runs at a
    // time: the first one is done with).
    h.runtime.cancel_task(&h.session, id).unwrap();
    let id = h.create_with(|r| r.scope = scope_of(2));
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    let steps = h.persistence.load_steps(id).unwrap();
    assert_eq!(steps[0].backend.as_deref(), Some("default"));
    assert_eq!(steps[0].model.as_deref(), Some("small"));
    assert_eq!(
        steps[0].transport,
        Some(crate::native::DecisionTransport::Envelope)
    );
    assert_eq!(default.requests().len(), 1);
    assert_eq!(deep.requests().len(), 2, "still the two of the first task");
}

/// REQ-MDL-010 (RFC 015, Amendment 1, item 4): the task's backend is resolved by its name at
/// every start, not once: a new process that made another adapter for the name is the one the
/// next request goes to, and nothing comes from `default`.
#[test]
fn req_mdl_010_a_restart_resolves_the_backend_by_name_again() {
    let default = Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "d")]));
    let first = Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "a")]));
    let second = Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "b")]));
    let directory = Directory::new(&[("default", &default), ("deep", &first)]);
    let h = with(
        Harness::with_backend(
            Arc::clone(&default),
            Policy::shipped(),
            vec![],
            crate::limits::LIM_C1_CONTEXT_CHARS,
        ),
        &directory,
        deep_on("deep"),
    );
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|r| r.effort = Some(EffortLevel::Deep));
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(first.requests().len(), 1);
    // A restart: the process that reads the file makes its own adapter for the name.
    directory.replace("deep", &second);
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(first.requests().len(), 1, "the old adapter is not asked");
    assert_eq!(second.requests().len(), 1, "the one the name gives now is");
    assert!(
        default.requests().is_empty(),
        "the backend named `default` never gets a task that is not on it"
    );
}

/// REQ-MDL-010 (external design §20): a backend removed from the file refuses the start, naming
/// the backend and the configured ones; the task stays as it was (no state change, no event, the
/// approval and the question still pending); nothing moves it to another backend, and no request
/// is sent anywhere. The file written again, the task starts.
#[test]
fn req_mdl_010_a_backend_removed_from_the_file_refuses_the_start_and_the_task_stays() {
    let default = Arc::new(ScriptedBackend::with_decisions(vec![]));
    let deep = Arc::new(ScriptedBackend::with_decisions(vec![
        act("write_file", "a"),
        json!({"decision": "ask_user", "question": "Which?"}),
        act("read_file", "b"),
    ]));
    let directory = Directory::new(&[("default", &default), ("deep", &deep)]);
    let h = with(
        Harness::with_policy(vec![], approval_policy()),
        &directory,
        deep_on("deep"),
    );
    let refusal = |error: RuntimeError| {
        let RuntimeError::InvalidRequest { detail } = error else {
            panic!("{error:?}")
        };
        assert!(detail.contains("`deep`"), "{detail}");
        assert!(detail.contains("configured: `default`"), "{detail}");
        assert!(detail.contains("nothing moves a task"), "{detail}");
    };

    // Created and held; the file loses `deep`; the resume is refused, the task stays `created`.
    let held = h.create_with(|r| {
        r.hold = true;
        r.scope = scope_of(1);
        r.effort = Some(EffortLevel::Deep);
    });
    directory.remove("deep");
    let events = h.persistence.events().len();
    refusal(h.runtime.resume_task(&h.session, held, None).unwrap_err());
    assert_eq!(h.state(held), TaskState::Created);
    assert_eq!(h.persistence.events().len(), events, "no event is written");
    assert!(deep.requests().is_empty() && default.requests().is_empty());

    // The same for a task that waits for an approval, and for one that waits for an answer.
    directory.replace("deep", &deep);
    let approval = h.create_with(|r| {
        r.scope = scope_of(2);
        r.effort = Some(EffortLevel::Deep);
    });
    assert_eq!(h.step(approval).outcome, StepOutcome::Waited);
    let question = h.create_with(|r| {
        r.effort = Some(EffortLevel::Deep);
        r.scope = scope_of(3);
    });
    assert_eq!(h.step(question).outcome, StepOutcome::Waited);
    let sent = deep.requests().len();
    directory.remove("deep");
    let events = h.persistence.events().len();
    refusal(
        h.runtime
            .approve_action(&h.session, approval, None)
            .unwrap_err(),
    );
    refusal(
        h.runtime
            .reject_action(&h.session, approval, None, Some(false))
            .unwrap_err(),
    );
    refusal(
        h.runtime
            .answer_question(&h.session, question, "main")
            .unwrap_err(),
    );
    assert_eq!(
        h.state(approval),
        TaskState::WaitingUser {
            wait_kind: WaitKind::Approval
        }
    );
    assert_eq!(
        h.state(question),
        TaskState::WaitingUser {
            wait_kind: WaitKind::Question
        }
    );
    assert_eq!(h.persistence.events().len(), events, "no event is written");
    assert_eq!(deep.requests().len(), sent, "nothing was sent");
    assert!(default.requests().is_empty(), "and nothing moved");
    // A rejection that halts needs no backend: it only ends the task's wait.
    h.runtime
        .reject_action(&h.session, approval, None, Some(true))
        .unwrap();
    assert!(matches!(h.state(approval), TaskState::Halted { .. }));

    // A task that is running when its backend is gone is halted `backend_unavailable`, with
    // `halt_detail` `backend_not_configured` naming it and the configured ones, and not left
    // `running`: nothing is sent (external design §19.4).
    directory.replace("deep", &deep);
    let running = h.create_with(|r| {
        r.effort = Some(EffortLevel::Deep);
        r.scope = scope_of(4);
    });
    directory.remove("deep");
    let sent = deep.requests().len();
    let result = futures_block(h.runtime.run_step(running)).unwrap();
    assert_eq!(result.outcome, StepOutcome::Halted);
    let state = h.state(running);
    assert_eq!(
        state,
        TaskState::Halted {
            halt_reason: HaltReason::BackendUnavailable,
            halt_detail: Some(HaltDetail::BackendNotConfigured {
                backend: "deep".into(),
                configured: vec!["default".into()],
            }),
        }
    );
    // The state change says it too, in the event, and no step was made.
    let changed: Vec<_> = h
        .persistence
        .events()
        .into_iter()
        .filter(|e| e.task_id == Some(running))
        .filter_map(|e| match e.event {
            crate::event::Event::TaskStateChanged {
                reason: Some(reason),
                halt_detail,
                ..
            } => Some((reason, halt_detail)),
            _ => None,
        })
        .collect();
    let detail = HaltDetail::BackendNotConfigured {
        backend: "deep".into(),
        configured: vec!["default".into()],
    };
    assert_eq!(
        changed,
        [(HaltReason::BackendUnavailable, Some(detail.clone()))]
    );
    // And the task as a script reads it.
    let json = serde_json::to_value(h.runtime.get_task(running).unwrap().task).unwrap();
    assert_eq!(json["halt_reason"], "backend_unavailable");
    assert_eq!(
        json["halt_detail"],
        json!({"rule": "backend_not_configured", "backend": "deep", "configured": ["default"]})
    );
    assert!(h.persistence.load_steps(running).unwrap().is_empty());
    assert_eq!(deep.requests().len(), sent, "nothing was sent");
    assert!(default.requests().is_empty(), "and nothing moved");
    // The file fixed, `resume` goes on: the task is running again, on its own backend.
    directory.replace("deep", &deep);
    h.runtime.resume_task(&h.session, running, None).unwrap();
    assert_eq!(h.state(running), TaskState::Running);
    assert_eq!(h.step(running).outcome, StepOutcome::Continued);
    assert_eq!(deep.requests().len(), sent + 1, "sent to its own backend");

    // The file has it again: the held task starts, on that backend (one task runs at a time).
    h.runtime.cancel_task(&h.session, running).unwrap();
    directory.replace("deep", &deep);
    h.runtime.resume_task(&h.session, held, None).unwrap();
    assert_eq!(h.state(held), TaskState::Running);
}

/// REQ-MDL-010 (external design §5.2, §19.4, review of 015c-6): a task that is `running` when its
/// backend is configured but cannot be made (a key by reference that the environment does not
/// hold) is halted `backend_unavailable`, with `halt_detail` `backend_unusable` and the reason:
/// in the task, in its state change, and as a script reads it. Nothing is sent, nothing moves it
/// to another backend and it is not left `running`; with the environment fixed, `resume` goes on.
#[test]
fn req_mdl_010_a_running_task_whose_backend_cannot_be_made_is_halted_backend_unusable() {
    let default = Arc::new(ScriptedBackend::with_decisions(vec![]));
    let deep = Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "a")]));
    let directory = Directory::new(&[("default", &default), ("deep", &deep)]);
    let h = with(
        Harness::with_policy(vec![], approval_policy()),
        &directory,
        deep_on("deep"),
    );
    let running = h.create_with(|r| {
        r.effort = Some(EffortLevel::Deep);
        r.scope = scope_of(1);
    });
    assert_eq!(h.state(running), TaskState::Running);
    let reason = "the environment variable DEEP_KEY is not set";
    directory.set_unusable("deep", Some(reason));
    let result = futures_block(h.runtime.run_step(running)).unwrap();
    assert_eq!(result.outcome, StepOutcome::Halted);
    let detail = HaltDetail::BackendUnusable {
        backend: "deep".into(),
        reason: reason.into(),
    };
    assert_eq!(
        h.state(running),
        TaskState::Halted {
            halt_reason: HaltReason::BackendUnavailable,
            halt_detail: Some(detail.clone()),
        },
        "halted, and not left running"
    );
    // The state change says it too, in the event, and no step was made.
    let changed: Vec<_> = h
        .persistence
        .events()
        .into_iter()
        .filter(|e| e.task_id == Some(running))
        .filter_map(|e| match e.event {
            crate::event::Event::TaskStateChanged {
                reason: Some(reason),
                halt_detail,
                ..
            } => Some((reason, halt_detail)),
            _ => None,
        })
        .collect();
    assert_eq!(changed, [(HaltReason::BackendUnavailable, Some(detail))]);
    // And the task as a script reads it.
    let json = serde_json::to_value(h.runtime.get_task(running).unwrap().task).unwrap();
    assert_eq!(json["halt_reason"], "backend_unavailable");
    assert_eq!(
        json["halt_detail"],
        json!({"rule": "backend_unusable", "backend": "deep", "reason": reason})
    );
    assert!(h.persistence.load_steps(running).unwrap().is_empty());
    assert!(
        deep.requests().is_empty() && default.requests().is_empty(),
        "nothing was sent, and nothing moved"
    );
    // While it cannot be made, `resume` is refused and the task stays halted.
    let refused = h
        .runtime
        .resume_task(&h.session, running, None)
        .unwrap_err();
    assert!(
        matches!(&refused, RuntimeError::InvalidRequest { detail } if detail.contains(reason)),
        "{refused:?}"
    );
    assert!(matches!(h.state(running), TaskState::Halted { .. }));
    // The environment fixed, `resume` goes on, on its own backend.
    directory.set_unusable("deep", None);
    h.runtime.resume_task(&h.session, running, None).unwrap();
    assert_eq!(h.state(running), TaskState::Running);
    assert_eq!(h.step(running).outcome, StepOutcome::Continued);
    assert_eq!(deep.requests().len(), 1, "sent to its own backend");
    assert!(default.requests().is_empty());
}

/// REQ-SEC-010 (external design §20): a level on a remote backend under a profile without
/// `external` is refused at creation, as a remote `default` is; and a task whose backend turns
/// remote between two starts halts `policy` at its next step, naming that backend and the
/// profile, before any request is sent.
#[test]
fn req_sec_010_a_remote_level_under_a_profile_without_external_halts_policy() {
    let default = Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "d")]));
    let deep = Arc::new(ScriptedBackend::with_decisions(vec![act("read_file", "a")]));
    let directory = Directory::new(&[("default", &default), ("deep", &deep)]);
    let h = with(
        Harness::with_backend(
            Arc::clone(&default),
            Policy::shipped(),
            vec![],
            crate::limits::LIM_C1_CONTEXT_CHARS,
        ),
        &directory,
        deep_on("deep"),
    );
    // A remote level is refused at creation: the profile has `external = false`.
    deep.set_remote(true);
    let error = h
        .runtime
        .create_task(
            &h.session,
            CreateTask {
                effort: Some(EffortLevel::Deep),
                ..create_request()
            },
        )
        .unwrap_err();
    assert!(
        matches!(&error, RuntimeError::Denied { detail } if detail.contains("forbids external communication")),
        "{error:?}"
    );
    nothing_made(&h, "a remote level");
    assert!(
        h.runtime.creation_forbidden(None, "deep").unwrap(),
        "the question a command asks before it contacts the backend"
    );
    assert!(!h.runtime.creation_forbidden(None, "default").unwrap());

    // A task made on a local `deep`, whose backend is remote at its next start, halts `policy`.
    deep.set_remote(false);
    let id = h.create_with(|r| {
        r.hold = true;
        r.effort = Some(EffortLevel::Deep);
    });
    deep.set_remote(true);
    h.runtime.resume_task(&h.session, id, None).unwrap();
    let result = h.step(id);
    assert_eq!(result.outcome, StepOutcome::Halted);
    assert_eq!(
        result.state,
        TaskState::Halted {
            halt_reason: HaltReason::Policy,
            halt_detail: Some(crate::task::HaltDetail::RemoteBackendForbidden {
                backend: "deep".into(),
                profile: "workspace".into(),
            }),
        }
    );
    assert!(deep.requests().is_empty(), "nothing was sent");
    assert!(default.requests().is_empty());
}

/// REQ-MDL-010, REQ-PERS-015 (external design §20): a backend that serves another model under
/// the same name goes on, and the start can say both models once: the model the task last ran
/// with is the last step's, so each change is told once; a backend that names no model changes
/// nothing.
#[test]
fn req_mdl_010_a_model_changed_under_the_same_name_is_told_once_and_the_task_goes_on() {
    let default = Arc::new(ScriptedBackend::with_decisions(vec![
        act("read_file", "a"),
        act("read_file", "b"),
        act("read_file", "c"),
    ]));
    default.set_model(Some("m1"));
    let directory = Directory::new(&[("default", &default)]);
    let h = with(
        Harness::with_backend(
            Arc::clone(&default),
            Policy::shipped(),
            vec![],
            crate::limits::LIM_C1_CONTEXT_CHARS,
        ),
        &directory,
        Levels::shipped(),
    );
    for _ in 0..3 {
        h.tools
            .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    }
    let id = h.create();
    let task = || h.runtime.get_task(id).unwrap().task;
    assert_eq!(h.runtime.model_change(&task()).unwrap(), None, "no change");
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.runtime.model_change(&task()).unwrap(), None);

    // The engine serves another model now: the next start says both, and the task goes on.
    default.set_model(Some("m2"));
    let change = h.runtime.model_change(&task()).unwrap().unwrap();
    assert_eq!(
        (
            change.backend.as_str(),
            change.was.as_str(),
            change.now.as_str()
        ),
        ("default", "m1", "m2")
    );
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    let models: Vec<_> = h
        .persistence
        .load_steps(id)
        .unwrap()
        .into_iter()
        .map(|s| s.model)
        .collect();
    assert_eq!(models, [Some("m1".to_owned()), Some("m2".to_owned())]);
    // What the task cost says it used both models (REQ-OBS-010), each with its steps.
    let cost = h.runtime.get_task(id).unwrap().cost;
    assert_eq!(
        cost.backends
            .iter()
            .map(|b| (b.backend.as_str(), b.model.as_deref(), b.steps))
            .collect::<Vec<_>>(),
        [("default", Some("m1"), 1), ("default", Some("m2"), 1)]
    );
    assert_eq!((cost.steps_used, cost.steps_envelope), (2, 40));
    // The step recorded it, so the change is not told again.
    assert_eq!(h.runtime.model_change(&task()).unwrap(), None);
    // A backend that names no model has nothing to compare.
    default.set_model(None);
    assert_eq!(h.runtime.model_change(&task()).unwrap(), None);
}

/// REQ-TASK-013, LIM-C3: `recent_steps` is read from the task. A task at 2 compresses from its
/// third step back (and summarizes what is older), a task at the shipped 6 sends the requests a
/// task of 0.12 sent (the same bytes as a task whose record has no level), and a flag-less task
/// with no key sends them too.
#[test]
fn req_task_013_recent_steps_is_read_from_the_task() {
    const N: usize = 14;
    // The sizes the tight budget is derived from, as `req_ctx_003_budget_rules_and_retained_set`.
    let generous = Harness::new(context_scenario(N));
    script_context_outcomes(&generous, N);
    let id = generous.create_with(|r| r.profile = Some("read_only".into()));
    for _ in 0..N {
        generous.step(id);
    }
    let last = request_chars(&generous, N - 1);
    let first = request_chars(&generous, 0);
    let budget = first + (last - first) / 3;

    let requests_of = |recent: Option<u32>, forget: bool| {
        let default = Arc::new(ScriptedBackend::with_decisions(context_scenario(N)));
        let directory = Directory::new(&[("default", &default)]);
        let mut levels = Levels::shipped();
        levels.standard.recent_steps = recent;
        let h = with(
            Harness::with_backend(default, Policy::shipped(), vec![], budget),
            &directory,
            levels,
        );
        script_context_outcomes(&h, N);
        let id = h.create_with(|r| r.profile = Some("read_only".into()));
        if forget {
            // A record written by 0.12: no level, no envelope.
            h.persistence.forget_effort(id);
        }
        for _ in 0..N {
            assert_eq!(h.step(id).outcome, StepOutcome::Continued);
        }
        let sent: Vec<_> = h
            .backend
            .requests()
            .into_iter()
            .map(|r| r.messages)
            .collect();
        (h, id, sent)
    };

    let (_, _, today) = requests_of(None, true);
    let (_, _, none) = requests_of(None, false);
    let (_, _, six) = requests_of(Some(6), false);
    assert_eq!(
        none, today,
        "a task of this release with no key sends what 0.12 sent"
    );
    assert_eq!(six, today, "recent_steps 6 sends the requests of today");

    let (h, id, two) = requests_of(Some(2), false);
    assert_ne!(two, today, "a task at 2 sends other requests");
    let mut compressed = 0;
    for (k, messages) in two.iter().enumerate() {
        // Steps before the request: k. The last `recent_steps` stay whole; the older ones are
        // summaries or dropped.
        let before = k;
        match history_segment(messages) {
            Some((summarized, dropped, _)) => {
                compressed += 1;
                assert_eq!(
                    summarized + dropped,
                    before.saturating_sub(2),
                    "request {k}: a step older than the last two is not whole"
                );
                assert_eq!(
                    measures(&h, id, k).context.compressed_steps as usize,
                    summarized
                );
            }
            None => {
                let m = measures(&h, id, k);
                assert_eq!(m.context.compressed_steps, 0, "request {k}");
            }
        }
    }
    assert!(compressed > 0, "the tight budget compressed nothing");
    // The first request that compresses is the one that has a third step back: under the
    // shipped 6 it is the eighth.
    let first_compressed =
        |sent: &[Vec<crate::spi::Message>]| sent.iter().position(|m| history_segment(m).is_some());
    let at_two = first_compressed(&two).unwrap();
    let at_six = first_compressed(&today).unwrap();
    assert!(
        at_two >= 3,
        "compression starts from the third step back: {at_two}"
    );
    assert!(at_two < at_six, "{at_two} before {at_six}");
}

/// REQ-TASK-013 (external design §19.4): a record written before 0.13 has no level and reads as
/// `standard`, chosen by default, with the envelope of its own budgets; every new field is absent
/// from a record that has nothing to say, and a record that has them reads back whole.
#[test]
fn req_task_013_a_record_of_0_12_reads_as_standard_and_empty_fields_are_absent() {
    let h = Harness::new(vec![act("read_file", "a")]);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|r| {
        r.budgets = Some(Budgets {
            steps: 25,
            ..Budgets::default()
        })
    });
    h.persistence.forget_effort(id);
    let task = h.runtime.get_task(id).unwrap().task;
    assert_eq!(task.effort, None);
    let json = serde_json::to_value(&task).unwrap();
    assert!(json.get("effort").is_none(), "{json}");
    let read = task.effort_in_force();
    assert_eq!(read.level, EffortLevel::Standard);
    assert_eq!(read.chosen_by, ChosenBy::Default);
    assert_eq!(read.envelope.backend, "default");
    assert_eq!(read.envelope.steps, 25, "the envelope of its own budgets");
    assert_eq!(
        read.envelope.deadline_ms,
        crate::limits::LIM_T5_DEADLINE_MS,
        "and its own deadline"
    );
    assert_eq!(read.envelope.context_chars, None);
    assert_eq!(task.recent_steps(), 6);
    // A 0.12 record, parsed from its JSON, is the same task.
    let parsed: crate::task::Task = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, task);

    // A step that sent nothing has no backend, model or transport to say.
    let step = crate::step::Step::start(
        crate::ids::StepId::new(&crate::ids::ScriptedIds::default()),
        id,
        0,
        task.created_at,
    );
    let json = serde_json::to_value(&step).unwrap();
    for key in ["backend", "model", "transport"] {
        assert!(
            json.get(key).is_none(),
            "{key} is absent when empty: {json}"
        );
    }
    // A step of 0.12 reads: its JSON has none of the three.
    let parsed: crate::step::Step = serde_json::from_value(json).unwrap();
    assert_eq!(
        (parsed.backend, parsed.model, parsed.transport),
        (None, None, None)
    );

    // The creation event of 0.12 has neither flag nor level: it reads, with none.
    let old: crate::event::Event =
        serde_json::from_value(json!({"event": "task.created"})).unwrap();
    assert_eq!(
        old,
        crate::event::Event::TaskCreated {
            plan_first: false,
            effort: None
        }
    );
    // A record of this release says the level in the task and in the event.
    let id = h.create_with(|r| {
        r.hold = true;
        r.scope = ScopeSpec {
            root: "/home/u/another".into(),
            include: vec![],
        };
    });
    let json = serde_json::to_value(h.runtime.get_task(id).unwrap().task).unwrap();
    assert_eq!(json["effort"]["level"], "standard", "{json}");
    assert_eq!(json["effort"]["chosen_by"], "default");
    assert_eq!(json["effort"]["envelope"]["backend"], "default");
}

/// REQ-OBS-010: the time a task took. A task that is over is stamped when the step that ended it
/// began, so its own request would be missing from its time: the end is the later of that and the
/// end of the last model request, and the model's time never exceeds the time the task took. A
/// task that is not over runs until now.
#[test]
fn req_obs_010_the_time_of_a_finished_task_includes_its_last_request() {
    let h = Harness::new(vec![json!({"decision": "complete", "summary": "done"})]);
    let id = h.create();
    assert_eq!(h.step(id).outcome, StepOutcome::Completed);
    let task = h.runtime.get_task(id).unwrap().task;
    assert!(task.state.is_terminal());
    let mut steps = h.persistence.load_steps(id).unwrap();
    // The request took 5 s; the clock, which the fake does not move, stamped the task at its start.
    steps[0].measures.as_mut().unwrap().model_ms = Some(5000);
    let cost = crate::effort::Cost::of(&task, &steps, task.updated_at);
    assert_eq!(cost.model_ms, 5000);
    assert_eq!(cost.elapsed_ms, 5000, "the request's end, after the stamp");
    // Without a measure the stamp is the end.
    steps[0].measures.as_mut().unwrap().model_ms = None;
    let cost = crate::effort::Cost::of(&task, &steps, task.updated_at);
    assert_eq!(cost.elapsed_ms, 0);

    // A task that is not over runs until now.
    let h = Harness::new(vec![act("read_file", "a")]);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create();
    h.step(id);
    h.clock.advance_millis(7000);
    let cost = h.runtime.get_task(id).unwrap().cost;
    assert_eq!(cost.elapsed_ms, 7000);
    assert_eq!(cost.deadline_envelope_ms, crate::limits::LIM_T5_DEADLINE_MS);
}

/// REQ-TASK-013: a task of 0.12 (no level in its record) and a task of this release at `standard`
/// send the same requests to the same backend, byte for byte: a valid configuration of 0.12 runs
/// as it did.
#[test]
fn req_task_013_standard_sends_what_a_task_without_a_level_sent() {
    let run = |forget: bool, effort: Option<EffortLevel>| {
        let h = Harness::new(vec![act("read_file", "a"), act("read_file", "b")]);
        h.tools.script(
            "read_file",
            ExecutionOutcome::succeeded(json!({"content": "x"}), 1),
        );
        h.tools.script(
            "read_file",
            ExecutionOutcome::succeeded(json!({"content": "y"}), 1),
        );
        let id = h.create_with(|r| r.effort = effort);
        if forget {
            h.persistence.forget_effort(id);
        }
        h.step(id);
        h.step(id);
        h.backend
            .requests()
            .into_iter()
            .map(|r| serde_json::to_vec(&r.messages).unwrap())
            .collect::<Vec<_>>()
    };
    let before = run(true, None);
    assert_eq!(before.len(), 2);
    assert_eq!(run(false, None), before, "no flag");
    assert_eq!(
        run(false, Some(EffortLevel::Standard)),
        before,
        "--effort standard"
    );
}

/// REQ-TASK-013: a fork takes its source's level unless `--effort` is given, its envelope resolved
/// from the configuration now (chosen by `fork`); a source of 0.12 reads as `standard`.
#[test]
fn req_task_013_a_fork_takes_its_sources_level() {
    let default = Arc::new(ScriptedBackend::with_decisions(vec![
        json!({"decision": "give_up", "reason": "x"}),
    ]));
    let directory = Directory::new(&[("default", &default)]);
    let h = with(Harness::new(vec![]), &directory, Levels::shipped());
    let source = h.create_with(|r| {
        r.hold = true;
        r.effort = Some(EffortLevel::Quick);
    });
    h.runtime.cancel_task(&h.session, source).unwrap();
    let fork = h.create_with(|r| {
        r.hold = true;
        r.from_task = Some(source);
    });
    let recorded = effort_of(&h, fork);
    assert_eq!(recorded.level, EffortLevel::Quick);
    assert_eq!(recorded.chosen_by, ChosenBy::Fork);
    assert_eq!(recorded.envelope.steps, 22);
    // The flag over the source's.
    let explicit = h.create_with(|r| {
        r.hold = true;
        r.from_task = Some(source);
        r.effort = Some(EffortLevel::Standard);
        r.scope = ScopeSpec {
            root: "/home/u/second".into(),
            include: vec![],
        };
    });
    assert_eq!(effort_of(&h, explicit).level, EffortLevel::Standard);
    assert_eq!(effort_of(&h, explicit).chosen_by, ChosenBy::User);
    // A source of 0.12 is `standard`.
    h.persistence.forget_effort(source);
    let old = h.create_with(|r| {
        r.hold = true;
        r.from_task = Some(source);
        r.scope = ScopeSpec {
            root: "/home/u/third".into(),
            include: vec![],
        };
    });
    assert_eq!(effort_of(&h, old).level, EffortLevel::Standard);
    assert_eq!(effort_of(&h, old).chosen_by, ChosenBy::Fork);
}

/// REQ-OBS-010 (external design §19.4, review of 015c, item 2): the cost says both step counts.
/// The steps decided are every step the task finished; the steps counted against the budget are
/// the task's counter, and the step that ends a task (`complete`) is decided and not counted.
#[test]
fn req_obs_010_the_cost_names_the_steps_decided_and_the_steps_counted() {
    let h = Harness::new(vec![
        act("read_file", "a"),
        act("read_file", "b"),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    h.tools
        .script("read_file", ExecutionOutcome::succeeded(json!({}), 1));
    let id = h.create_with(|_| {});
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    assert_eq!(h.step(id).outcome, StepOutcome::Continued);
    let running = h.runtime.get_task(id).unwrap().cost;
    assert_eq!((running.steps_decided, running.steps_used), (2, 2));
    assert_eq!(h.step(id).outcome, StepOutcome::Completed);
    let cost = h.runtime.get_task(id).unwrap().cost;
    assert_eq!(
        (cost.steps_decided, cost.steps_used, cost.steps_envelope),
        (3, 2, 40),
        "the step that ends the task is decided and not counted"
    );
    let json = serde_json::to_value(&cost).unwrap();
    assert_eq!(
        (json["steps_decided"].as_u64(), json["steps_used"].as_u64()),
        (Some(3), Some(2))
    );
}
