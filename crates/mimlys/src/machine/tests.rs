//! The machine view (RFC 013 decision 8; REQ-OBS-009; external design §19.3), from what a scan of
//! the registry found: no file is read and no clock is, so the verdicts are values.

use super::*;
use crate::registry::{ENTRY_VERSION, Reaped, Seen};
use serde_json::json;

fn at(millis: i64) -> Instant {
    Instant::from_unix_millis(1_767_225_600_000 + millis).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn seen(
    stem: &str,
    kind: Kind,
    state: State,
    pid: u32,
    task: &str,
    limit: u32,
    gate: Option<&str>,
    since: i64,
) -> Seen {
    Seen {
        stem: stem.into(),
        entry: Entry {
            v: ENTRY_VERSION,
            kind,
            state,
            pid,
            data_dir: format!("/data/{pid}"),
            task_id: Some(task.into()),
            since: at(since),
            limit,
            key: gate.map(|_| "k1".to_owned()),
            url: gate.map(str::to_owned),
        },
    }
}

fn scan() -> Scan {
    Scan {
        live: vec![
            seen("a", Kind::Slot, State::Held, 10, "t10", 2, None, 100),
            seen("b", Kind::Slot, State::Held, 11, "t11", 2, None, 200),
            seen("c", Kind::Slot, State::Waiting, 12, "t12", 2, None, 300),
            seen(
                "d",
                Kind::Gate,
                State::Held,
                13,
                "t13",
                1,
                Some("http://e:1"),
                400,
            ),
            seen(
                "e",
                Kind::Gate,
                State::Waiting,
                14,
                "t14",
                1,
                Some("http://e:1"),
                500,
            ),
        ],
        reaped: vec![Reaped {
            stem: "z".into(),
            entry: None,
            reason: "its holder is gone".into(),
        }],
    }
}

/// REQ-OBS-009: the slots taken are the slots held, counted across runtimes; the runtimes that
/// wait are not slots; a held gate is not a slot; the limit is the configuration's.
#[test]
fn req_obs_009_the_slots_taken_are_the_slots_held() {
    let machine = from_scan(&scan(), 2);
    assert_eq!(machine.slots, Slots { limit: 2, taken: 2 });
    assert_eq!(machine.stale_entries, 1);
    let none = from_scan(&Scan::default(), 3);
    assert_eq!(none.slots, Slots { limit: 3, taken: 0 });
    assert!(none.runtimes.is_empty() && none.engine_gates.is_empty());
}

/// REQ-OBS-009: each runtime that is executing or waiting is a row with its pid, data directory,
/// task and since when; `running` holds a slot or a gate, `waiting` names what it waits for and
/// the processes it waits behind, oldest first.
#[test]
fn req_obs_009_a_row_is_running_or_waiting_with_what_it_holds_or_waits_for() {
    let machine = from_scan(&scan(), 2);
    type Row = (u32, RowState, Option<Holds>, Option<WaitingFor>, Vec<u32>);
    let rows: Vec<Row> = machine
        .runtimes
        .iter()
        .map(|r| (r.pid, r.state, r.holds, r.waiting_for, r.behind.clone()))
        .collect();
    assert_eq!(
        rows,
        [
            (10, RowState::Running, Some(Holds::Slot), None, vec![]),
            (11, RowState::Running, Some(Holds::Slot), None, vec![]),
            (
                12,
                RowState::Waiting,
                None,
                Some(WaitingFor::Admission),
                vec![10, 11]
            ),
            (13, RowState::Running, Some(Holds::EngineGate), None, vec![]),
            (
                14,
                RowState::Waiting,
                None,
                Some(WaitingFor::Engine),
                vec![13]
            ),
        ]
    );
    let waiting = &machine.runtimes[2];
    assert_eq!(waiting.data_dir, "/data/12");
    assert_eq!(waiting.task_id.as_deref(), Some("t12"));
    assert_eq!(waiting.limit, Some(2));
    assert_eq!(waiting.since, at(300));
}

/// REQ-OBS-009: the JSON shape of the machine view, field by field. It is part of the contract
/// of `status --json` (`machine`); a field renamed here is a breaking change.
#[test]
fn req_obs_009_the_json_shape_of_the_machine_view() {
    let mut machine = from_scan(&scan(), 2);
    machine.registry = Some("/run/user/1000/mimlys".into());
    let value = serde_json::to_value(&machine).unwrap();
    assert_eq!(
        value,
        json!({
            "registry": "/run/user/1000/mimlys",
            "admission": {"state": "ready"},
            "slots": {"limit": 2, "taken": 2},
            "runtimes": [
                {"pid": 10, "data_dir": "/data/10", "task_id": "t10", "state": "running", "holds": "slot", "since": at(100)},
                {"pid": 11, "data_dir": "/data/11", "task_id": "t11", "state": "running", "holds": "slot", "since": at(200)},
                {"pid": 12, "data_dir": "/data/12", "task_id": "t12", "state": "waiting", "waiting_for": "admission", "behind": [10, 11], "limit": 2, "since": at(300)},
                {"pid": 13, "data_dir": "/data/13", "task_id": "t13", "state": "running", "holds": "engine_gate", "since": at(400)},
                {"pid": 14, "data_dir": "/data/14", "task_id": "t14", "state": "waiting", "waiting_for": "engine", "behind": [13], "limit": 1, "since": at(500)}
            ],
            "engine_gates": [
                {"url": "http://e:1", "in_flight": 1, "limit": 1, "holders": [13], "waiting": [14]}
            ],
            "stale_entries": 1
        })
    );
    let unavailable = view(Err("no home directory is known for this user".into()), 2);
    assert_eq!(
        serde_json::to_value(&unavailable).unwrap(),
        json!({
            "admission": {"state": "unavailable", "reason": "no home directory is known for this user"},
            "slots": {"limit": 2, "taken": 0},
            "runtimes": [],
            "engine_gates": [],
            "stale_entries": 0
        })
    );
}

/// REQ-OBS-009: what a task waits for is the waiting entry of its own data directory and task, with
/// the processes that hold what it waits for; another task's wait is not its own.
#[test]
fn req_obs_009_a_tasks_wait_is_found_by_its_data_directory_and_task() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Registry::new(
        dir.path().to_owned(),
        crate::registry::Bounds::default(),
        std::sync::Arc::new(mimlys_core::clock::FakeClock::default()),
    );
    let class = crate::registry::Class {
        kind: Kind::Slot,
        key: None,
        url: None,
        limit: 1,
    };
    let owner = |pid: u32, dir: &str| crate::registry::Owner {
        pid,
        data_dir: dir.into(),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _held = rt.block_on(async {
        let Ok(crate::registry::Take::Granted { handle, .. }) = registry
            .take(&class, Some("holder".into()), &owner(1, "/d1"), None)
            .await
        else {
            panic!("a free slot");
        };
        handle
    });
    let _waiter = rt.block_on(async {
        let Ok(crate::registry::Take::Busy { waiter, .. }) = registry
            .take(&class, Some("mine".into()), &owner(2, "/d2"), None)
            .await
        else {
            panic!("the slot is held");
        };
        waiter
    });
    let found = waiting_of(Ok(dir.path().to_owned()), "/d2", "mine").unwrap();
    assert_eq!(found.reason, "admission");
    assert_eq!(found.holders, [1]);
    assert_eq!((found.limit, found.taken), (1, 1));
    assert!(waiting_of(Ok(dir.path().to_owned()), "/d2", "other").is_none());
    assert!(waiting_of(Ok(dir.path().to_owned()), "/elsewhere", "mine").is_none());
    assert!(waiting_of(Err("none".into()), "/d2", "mine").is_none());
}
