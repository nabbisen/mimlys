//! A store with the words of a newer release in it (REQ-MAIN-006; external design §22; handoff
//! 004e, item 1): the commands that read show every such word by its name, marked `unknown to this
//! release`, in text and in `--json`, and every other record as it always was. The store is made
//! by `common::newer_store`; the commands are the real `mimlys` binary.

mod common;
use common::newer_store::*;
use serde_json::{Value, json};

/// REQ-MAIN-006: every reader succeeds on a store that holds the words of a newer release and
/// shows each by its name with the marker; every other record is shown.
#[test]
fn req_main_006_the_readers_show_every_word_with_its_marker_in_text() {
    let s = Newer::new();
    let env = &s.env;

    // `events`: the event by its name and its fields, and the halt reason inside a known event.
    let (code, out, err) = env.run(&["events"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(err.is_empty(), "{err}");
    assert!(
        out.contains(&format!("session.attached {MARKER}")) && out.contains("client=tui"),
        "{out}"
    );
    assert!(
        out.contains(&format!("reason=plan_required {MARKER}")),
        "{out}"
    );
    assert!(
        out.contains("task.created"),
        "known events are listed too: {out}"
    );
    let (code, out, _) = env.run(&["events", &s.event]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("session.attached"), "{out}");

    // `task show`: the state, the wait kind, the reasons, the record that cannot be read.
    for (task, field, word) in s.newer_tasks() {
        let (code, out, err) = env.run(&["task", "show", task]);
        assert_eq!(code, 0, "{field}: {out}\n{err}");
        assert!(err.is_empty(), "{err}");
        assert!(out.contains(task), "{out}");
        assert!(
            out.contains(&format!("`{word}` {MARKER}"))
                || out.contains(&format!("{word} {MARKER}")),
            "{field} `{word}` is not marked: {out}"
        );
        assert!(
            out.contains("goal"),
            "the task is shown, not only the word: {out}"
        );
    }
    let (_, out, _) = env.run(&["task", "show", &s.state]);
    assert!(
        out.contains(&format!("state     quarantined {MARKER}")),
        "{out}"
    );
    let (_, out, _) = env.run(&["task", "show", &s.action]);
    assert!(
        out.contains(&s.unreadable_action.to_string()) && out.contains("held_for_plan"),
        "{out}"
    );

    // `task list`: every task survives, each unknown state is marked.
    let (code, out, err) = env.run(&["task", "list"]);
    assert_eq!(code, 0, "{out}\n{err}");
    for task in [
        &s.plain, &s.wait, &s.state, &s.halt, &s.failed, &s.event, &s.action,
    ] {
        assert!(out.contains(task.as_str()), "{task} is not listed: {out}");
    }
    assert!(out.contains(&format!("quarantined {MARKER}")), "{out}");
    assert!(
        out.contains(&format!("wait_kind `plan` {MARKER}")),
        "the task whose wait kind is not known: {out}"
    );

    // `metrics`: by name, marked.
    let (code, out, err) = env.run(&["metrics"]);
    assert_eq!(code, 0, "{out}\n{err}");
    for line in [
        "events_by_name.session.attached",
        "tasks_by_state.quarantined",
        "halts_by_reason.plan_required",
    ] {
        let found = out
            .lines()
            .find(|l| l.starts_with(line))
            .unwrap_or_else(|| panic!("{line}: {out}"));
        assert!(found.contains(MARKER), "{found}");
    }
    let known = out
        .lines()
        .find(|l| l.starts_with("events_by_name.task.created"))
        .unwrap();
    assert!(!known.contains(MARKER), "{known}");

    // `status`: the task that waits for a kind it does not know, and the one in a state it does not.
    let (code, out, err) = env.run(&["status"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.lines()
            .any(|l| l.contains(&s.wait) && l.contains(&format!("`plan` {MARKER}"))),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.contains(&s.state) && l.contains(&format!("`quarantined` {MARKER}"))),
        "{out}"
    );

    // `task output`: the action it can read; the one it cannot, named, is refused with its word.
    let (code, out, err) = env.run(&["task", "output", &s.action]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains(&s.readable_action.to_string()), "{out}");
    let (code, out, err) = env.run(&["task", "output", &s.action, &s.readable_action.to_string()]);
    assert_eq!(code, 0, "{out}\n{err}");
    let (code, out, err) = env.run(&[
        "task",
        "output",
        &s.action,
        &s.unreadable_action.to_string(),
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(
        err.contains("held_for_plan") && err.contains(MARKER.trim_matches(['(', ')'])),
        "{err}"
    );
}

/// REQ-MAIN-006: in `--json` the field is as written, and the object that holds a word this
/// release does not know lists it under `unknown_to_this_release`, as `{field, word}`.
#[test]
fn req_main_006_the_readers_mark_every_word_in_json_by_one_rule() {
    let s = Newer::new();
    let env = &s.env;
    let words = |value: &Value| -> Vec<(String, String)> {
        value[MARKER_KEY]
            .as_array()
            .unwrap_or_else(|| panic!("no marker in {value}"))
            .iter()
            .map(|w| {
                (
                    w["field"].as_str().unwrap().to_owned(),
                    w["word"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };

    // events: the record as written, with the key.
    let (code, events) = env.json(&["events"]);
    assert_eq!(code, 0, "{events}");
    let events = events.as_array().unwrap();
    let newer: Vec<&Value> = events
        .iter()
        .filter(|e| e["record"].get(MARKER_KEY).is_some())
        .collect();
    assert_eq!(newer.len(), 2, "the event and the halt: {events:?}");
    let attached = newer
        .iter()
        .find(|e| e["record"]["event"] == "session.attached")
        .unwrap();
    assert_eq!(attached["record"]["client"], "tui", "the fields as written");
    assert_eq!(
        words(&attached["record"]),
        [("event".into(), "session.attached".into())]
    );
    let halted = newer
        .iter()
        .find(|e| e["record"]["event"] == "task.state_changed")
        .unwrap();
    assert_eq!(halted["record"]["reason"], "plan_required");
    assert_eq!(
        words(&halted["record"]),
        [("reason".into(), "plan_required".into())]
    );
    for event in events
        .iter()
        .filter(|e| e["record"].get(MARKER_KEY).is_none())
    {
        assert!(event["record"]["event"].is_string());
    }

    // task show: the task record as written, in `task`.
    let (code, shown) = env.json(&["task", "show", &s.wait]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["task"]["wait_kind"], "plan");
    assert_eq!(shown["task"]["goal"], "wait");
    assert_eq!(words(&shown["task"]), [("wait_kind".into(), "plan".into())]);
    let (code, shown) = env.json(&["task", "show", &s.state]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(
        words(&shown["task"]),
        [("state".into(), "quarantined".into())]
    );
    let (code, shown) = env.json(&["task", "show", &s.halt]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(
        words(&shown["task"]),
        [("halt_reason".into(), "plan_required".into())]
    );
    // A readable task with a record that is not: the record is listed, the view is whole.
    let (code, shown) = env.json(&["task", "show", &s.action]);
    assert_eq!(code, 0, "{shown}");
    assert!(shown["task"]["goal"].is_string() && shown.get(MARKER_KEY).is_none());
    let records = shown["unknown_records"].as_array().unwrap();
    assert_eq!(records.len(), 1, "{shown}");
    assert_eq!(records[0]["kind"], "action");
    assert_eq!(records[0]["key"], s.unreadable_action.to_string());
    assert_eq!(records[0]["record"]["status"], "held_for_plan");
    assert_eq!(
        words(&records[0]["record"]),
        [("status".into(), "held_for_plan".into())]
    );
    let (_, shown) = env.json(&["task", "show", &s.event]);
    assert_eq!(shown["unknown_records"][0]["kind"], "event");

    // task list: a row per task; the marker on the row that holds a word.
    let (code, list) = env.json(&["task", "list"]);
    assert_eq!(code, 0, "{list}");
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 7);
    let row = |id: &str| list.iter().find(|r| r["task_id"] == id).unwrap();
    assert_eq!(row(&s.state)["state"], "quarantined");
    assert_eq!(
        words(row(&s.state)),
        [("state".into(), "quarantined".into())]
    );
    assert_eq!(words(row(&s.wait)), [("wait_kind".into(), "plan".into())]);
    assert!(row(&s.plain).get(MARKER_KEY).is_none());

    // metrics: the groups name their unknown words.
    let (code, metrics) = env.json(&["metrics"]);
    assert_eq!(code, 0, "{metrics}");
    let marked = words(&metrics);
    for expected in [
        ("events_by_name", "session.attached"),
        ("halts_by_reason", "plan_required"),
        ("tasks_by_state", "quarantined"),
    ] {
        assert!(
            marked.contains(&(expected.0.into(), expected.1.into())),
            "{marked:?}"
        );
    }
    assert_eq!(metrics["events_by_name"]["session.attached"], 1);
    assert_eq!(metrics["halts_by_reason"]["plan_required"], 1);

    // status: the kind of the wait, and the state, as written, and the words listed.
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    let pending = status["pending"].as_array().unwrap();
    assert!(
        pending
            .iter()
            .any(|p| p[0] == s.wait.as_str() && p[1] == "plan"),
        "{status}"
    );
    assert!(
        status["unknown_state"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p[0] == s.state.as_str() && p[1] == "quarantined"),
        "{status}"
    );
    let marked = words(&status);
    assert!(
        marked.contains(&("pending".into(), "plan".into())),
        "{marked:?}"
    );
    assert!(
        marked.contains(&("unknown_state".into(), "quarantined".into())),
        "{marked:?}"
    );

    // task output: the readable action; the unreadable one is the refusal.
    let (code, out) = env.json(&["task", "output", &s.action]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["action_id"], s.readable_action.to_string());
    let (code, out) = env.json(&[
        "task",
        "output",
        &s.action,
        &s.unreadable_action.to_string(),
    ]);
    assert_eq!(code, 1);
    assert_eq!(out["error"]["kind"], "state", "{out}");
}

/// REQ-MAIN-006: a store of this release, with no word of a newer one, shows no marker anywhere,
/// in text or in `--json`: every record reads as it always did.
#[test]
fn req_main_006_a_store_of_this_release_has_no_marker_anywhere() {
    let env = Env::new();
    let a = env.new_task("alpha");
    let b = env.new_task("beta");
    env.write_task(
        &b,
        "halted",
        json!({"state": "halted", "halt_reason": "budget_steps"}),
    );
    let action = run_an_action(&env.data, &a, 0);
    let commands: Vec<Vec<&str>> = vec![
        vec!["events"],
        vec!["metrics"],
        vec!["status"],
        vec!["task", "list"],
        vec!["task", "show", &a],
        vec!["task", "show", &b],
        vec!["task", "output", &a],
    ];
    let _ = action;
    for args in commands {
        let (code, out, err) = env.run(&args);
        assert_eq!(code, 0, "{args:?}: {out}\n{err}");
        assert!(
            !out.contains("unknown to this release") && !err.contains("unknown to this release"),
            "{args:?}: {out}"
        );
        let mut json_args = vec!["--json"];
        json_args.extend_from_slice(&args);
        let (code, out, _) = env.run(&json_args);
        assert_eq!(code, 0, "{args:?}: {out}");
        assert!(!out.contains(MARKER_KEY), "{args:?}: {out}");
    }
}

/// REQ-MAIN-006: `events --follow` of a task in a state this release does not know ends, as it
/// does at a state that settles; it does not wait for a task it cannot tell moves.
#[test]
fn req_main_006_follow_ends_at_a_state_it_does_not_know() {
    let s = Newer::new();
    let (code, out, err) = s.env.run(&["events", &s.state, "--follow"]);
    assert_eq!(code, 0, "{out}\n{err}");
}

/// REQ-MAIN-006: a snapshot of a newer release is left out of a task's footprint and named, and
/// `task list`, `task show` and `status --storage` go on.
#[test]
fn req_main_006_a_snapshot_of_a_newer_release_is_left_out_and_named() {
    let env = Env::new();
    let task = env.new_task("snap");
    env.sql(
        "UPDATE snapshots SET reason = 'archive', record = json_set(record, '$.reason', 'archive') WHERE task_id = ?1",
        &[&task],
    );
    let (code, out, err) = env.run(&["task", "list"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains(&task), "{out}");
    let (code, out, err) = env.run(&["task", "show", &task]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("unknown   snapshot")
            && out.contains("reason `archive` (unknown to this release)"),
        "{out}"
    );
    let (code, shown) = env.json(&["task", "show", &task]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["unknown_records"][0]["kind"], "snapshot", "{shown}");
    assert_eq!(shown["snapshots"].as_array().unwrap().len(), 0, "{shown}");
    let (code, out, err) = env.run(&["status", "--storage"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("hold a word (unknown to this release)"),
        "{out}"
    );
    let (code, view) = env.json(&["status", "--storage"]);
    assert_eq!(code, 0, "{view}");
    assert_eq!(view["unknown_records"], 1, "{view}");
}
