//! A store with the words of a newer release in it (REQ-MAIN-006; external design §22; handoff
//! 004e, item 2): the commands that would change or run such a task refuse, naming the word, and
//! leave the store as it was; the start recovers the other tasks and leaves this one; `task prune`
//! leaves it alone. The store is made by `common::newer_store`; the commands are the real `mimlys`
//! binary.

mod common;
use common::newer_store::*;
use serde_json::json;

/// REQ-MAIN-006, item 2: each command that would change or run such a task refuses, naming the
/// word and saying that the release that wrote it is needed; the task's rows are as they were,
/// byte for byte. The commands are the ones `changes_task` derives; the kinds of record are the
/// ones the store keeps.
#[test]
fn req_main_006_every_writer_refuses_a_task_with_a_newer_word_and_changes_nothing() {
    let s = Newer::new();
    let env = &s.env;
    let writers = |task: &str| -> Vec<Vec<String>> {
        let t = task.to_owned();
        let v = |parts: &[&str]| parts.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
        vec![
            v(&["task", "resume", &t]),
            v(&["approve", &t]),
            v(&["reject", &t]),
            v(&["answer", &t, "yes"]),
            v(&["grant", &t, "tool", "write_file"]),
            v(&["revoke", &t, "00000000-0000-7000-8000-000000000001"]),
            v(&["rollback", &t]),
            v(&["promote", &t]),
            v(&["promote", &t, "--patch"]),
            v(&["task", "delete", &t, "--yes"]),
            v(&["task", "cancel", &t]),
            v(&["task", "pause", &t]),
            v(&["task", "new", "a fork", "--from-task", &t, "--hold"]),
            v(&["run", "a fork", "--from-task", &t]),
        ]
    };
    for (task, field, word) in s.newer_tasks() {
        let before = env.rows_of(task);
        for args in writers(task) {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let (code, out, err) = env.run(&args);
            assert_eq!(code, 1, "{args:?}: {out}\n{err}");
            assert!(err.contains("error (state)"), "{args:?}: {err}");
            assert!(
                err.contains(&format!("`{word}`")) && err.contains("unknown to this release"),
                "{args:?} names no word `{word}` ({field}): {err}"
            );
            assert!(
                err.contains("the release of mimlys that wrote it is needed"),
                "{args:?}: {err}"
            );
            assert_eq!(
                env.rows_of(task),
                before,
                "{args:?} changed the task's rows"
            );
        }
        // The same, in JSON: kind `state`, the word in the message.
        let (code, error) = env.json(&["task", "resume", task]);
        assert_eq!(code, 1);
        assert_eq!(error["error"]["kind"], "state", "{error}");
        assert!(
            error["error"]["message"].as_str().unwrap().contains(word),
            "{error}"
        );
        assert_eq!(env.rows_of(task), before);
    }
    // And a task that holds no such word is not refused by it.
    let (code, out, err) = env.run(&["task", "cancel", &s.plain]);
    assert_eq!(code, 0, "{out}\n{err}");
}

/// REQ-MAIN-006, item 2: a start that finds such tasks recovers the others and leaves these as
/// they are, saying so once each, and does not fail.
#[test]
fn req_main_006_the_start_recovers_the_others_and_leaves_a_newer_task_as_it_is() {
    let s = Newer::new();
    let env = &s.env;
    // A task found running by the start: it holds an event of a newer release.
    let running_newer = env.new_task("running-newer");
    env.write_task(&running_newer, "running", json!({"state": "running"}));
    env.write_event(
        &running_newer,
        3,
        "session.attached",
        json!({"client": "tui"}),
    );
    // And one that holds nothing of the kind, found running too.
    let running = env.new_task("running");
    env.write_task(&running, "running", json!({"state": "running"}));
    let untouched: Vec<(&str, String)> = [
        s.wait.as_str(),
        s.state.as_str(),
        s.halt.as_str(),
        s.failed.as_str(),
        s.event.as_str(),
        s.action.as_str(),
        running_newer.as_str(),
    ]
    .into_iter()
    .map(|t| (t, env.rows_of(t)))
    .collect();

    // Any command that writes starts the store and recovers it.
    let (code, out, err) = env.run(&["task", "cancel", &s.plain]);
    assert_eq!(code, 0, "{out}\n{err}");

    // The task that held nothing of the kind was recovered: it is `halted`, recovered.
    let (_, shown) = env.json(&["task", "show", &running]);
    assert_eq!(shown["task"]["state"], "halted", "{shown}");
    assert_eq!(shown["task"]["halt_reason"], "recovered", "{shown}");
    // The others are as they were, byte for byte.
    for (task, rows) in &untouched {
        assert_eq!(&env.rows_of(task), rows, "recovery changed task {task}");
    }
    // Said once each: a line of the start's log names the task.
    for (task, what) in [
        (&running_newer, "session.attached"),
        (&s.wait, "plan"),
        (&s.state, "quarantined"),
        (&s.halt, "plan_required"),
        (&s.failed, "daemon_lost"),
        (&s.action, "held_for_plan"),
    ] {
        let lines: Vec<&str> = err.lines().filter(|l| l.contains(task.as_str())).collect();
        // Tasks the start does not visit (halted, failed, an action's status) are not named by
        // it; the ones it visits, or cannot classify, are named exactly once.
        assert!(lines.len() <= 1, "{task}: {lines:?}");
        if let Some(line) = lines.first() {
            assert!(
                line.contains(what) && line.contains("left as it is"),
                "{line}"
            );
        }
    }
    for task in [&running_newer, &s.wait, &s.state] {
        assert_eq!(
            err.lines().filter(|l| l.contains(task.as_str())).count(),
            1,
            "{task} is said once: {err}"
        );
    }
}

/// REQ-MAIN-006: `task prune` leaves a task that holds a word of a newer release alone and says
/// so, and does not fail.
#[test]
fn req_main_006_prune_leaves_a_newer_task_alone_and_says_so() {
    let s = Newer::new();
    let env = &s.env;
    env.write_task(
        &s.event,
        "completed",
        json!({"state": "completed", "completion": "verified"}),
    );
    let before = env.rows_of(&s.event);
    let (code, out, err) = env.run(&["task", "prune", "--yes"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains(&format!("left alone: task {}", s.event)) && out.contains("session.attached"),
        "{out}"
    );
    assert_eq!(env.rows_of(&s.event), before);
    let (code, value) = env.json(&["task", "prune", "--yes"]);
    assert_eq!(code, 0, "{value}");
    let left = value["newer_tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["task_id"] == s.event.as_str())
        .unwrap_or_else(|| panic!("the task is not named: {value}"));
    assert_eq!(left["record"], "event");
    assert_eq!(
        left[MARKER_KEY],
        json!([{"field": "event", "word": "session.attached"}])
    );
}

/// REQ-MAIN-006: a task whose snapshot is of a newer release is one that a command which would
/// change it refuses, naming the word.
#[test]
fn req_main_006_a_task_with_a_newer_snapshot_is_refused_by_a_writer() {
    let env = Env::new();
    let task = env.new_task("snap");
    env.sql(
        "UPDATE snapshots SET reason = 'archive', record = json_set(record, '$.reason', 'archive') WHERE task_id = ?1",
        &[&task],
    );
    // The task itself is still one to change: the snapshot is the task's record.
    let (code, _, err) = env.run(&["task", "cancel", &task]);
    assert_eq!(code, 1);
    assert!(err.contains("`archive`"), "{err}");
}
