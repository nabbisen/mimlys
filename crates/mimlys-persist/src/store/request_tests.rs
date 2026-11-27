//! The request id in the store (handoff 014b, item 6; external design §4, §17.1): an event carries
//! the id of the request whose change wrote it, in its envelope, with no change to the schema;
//! the event is found again by that id, through the index the store already has, and a record
//! written without it reads as before. Judged by what the store returns, and by the plan the
//! database makes of the two lookups.

use super::tests::Fx;
use crate::sqlite::{SCHEMA_VERSION, open};
use mimlys_core::event::Event;
use mimlys_core::ids::{RequestId, ScriptedIds};
use mimlys_core::spi::{Persistence, PersistenceRead};

fn request(n: u64) -> RequestId {
    RequestId::new(&ScriptedIds::starting_at(50_000 + n))
}

/// A task created under `created` with a grant event under `granted`, on a real store; the
/// events of the task carry what the requests wrote.
struct Made {
    store: crate::store::Store,
    _dir: tempfile::TempDir,
    task: mimlys_core::task::Task,
}

fn made(fx: &Fx, created: Option<RequestId>, granted: Option<RequestId>) -> Made {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("data")).unwrap();
    let task = fx.task();
    let mut event = fx.event(
        &task,
        Event::TaskCreated {
            plan_first: false,
            effort: None,
        },
    );
    event.request_id = created;
    store.create_task(&task, &[], &event).unwrap();
    let mut event = fx.event(&task, Event::TaskPauseRequested);
    event.request_id = granted;
    store.append_event(&event).unwrap();
    Made {
        store,
        _dir: dir,
        task,
    }
}

/// External design §4: the first event a request's change wrote is found by the request's id in
/// its task's events; a request that wrote nothing, and any other id, are not found.
#[test]
fn req_arch_004_an_event_is_found_by_the_id_of_the_request_that_wrote_it() {
    let fx = Fx::new();
    let (a, b) = (request(1), request(2));
    let m = made(&fx, Some(a), Some(b));
    let found = m
        .store
        .event_of_request(Some(m.task.task_id), b)
        .unwrap()
        .expect("recorded");
    assert_eq!(found.record.event.name(), "task.pause_requested");
    assert_eq!(found.record.request_id, Some(b));
    assert_eq!(found.record.task_id, Some(m.task.task_id));
    // The position is the event's own, as `events_since` gives it.
    let all = m.store.events_since(0, Some(m.task.task_id), 10).unwrap();
    assert_eq!(found.position, all[1].position);
    // An id nothing recorded, and a task that is not the event's.
    assert!(
        m.store
            .event_of_request(Some(m.task.task_id), request(9))
            .unwrap()
            .is_none()
    );
    let other = fx.task();
    assert!(
        m.store
            .event_of_request(Some(other.task_id), b)
            .unwrap()
            .is_none(),
        "the search is of the task's own events"
    );
}

/// A request that has no task yet to name, a creation, is found through the first event of each
/// task, which is its `task.created`: and only through it.
#[test]
fn req_arch_004_a_creation_is_found_through_the_first_event_of_the_tasks() {
    let fx = Fx::new();
    let (created, later) = (request(1), request(2));
    let m = made(&fx, Some(created), Some(later));
    let found = m
        .store
        .event_of_request(None, created)
        .unwrap()
        .expect("the creation");
    assert_eq!(found.record.event.name(), "task.created");
    assert_eq!(found.record.task_id, Some(m.task.task_id));
    // An id that a later event of a task carries is not a creation's.
    assert!(m.store.event_of_request(None, later).unwrap().is_none());
    assert!(
        m.store
            .event_of_request(None, request(9))
            .unwrap()
            .is_none()
    );
}

/// Items 6 and the compatibility rule: the id is a field of the record's JSON, and the schema is
/// as it was; a record written without it reads as before and is found by no id.
#[test]
fn req_arch_004_the_id_is_in_the_record_and_the_schema_is_unchanged() {
    let fx = Fx::new();
    let id = request(1);
    let m = made(&fx, Some(id), None);
    let conn = rusqlite::Connection::open_with_flags(
        m._dir.path().join("data/mimlys.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let texts: Vec<String> = conn
        .prepare("SELECT record FROM events ORDER BY position")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        texts[0].contains(&format!("\"request_id\":\"{id}\"")),
        "{}",
        texts[0]
    );
    assert!(
        !texts[1].contains("request_id"),
        "an event no request wrote has none: {}",
        texts[1]
    );
    assert_eq!(
        m.store.schema_version().unwrap(),
        SCHEMA_VERSION,
        "no migration"
    );
    assert_eq!(SCHEMA_VERSION, 4, "the schema 0.13.0 reads");
}

/// Item 6: the lookups need no new index: the task's search runs on `events_task`, and the
/// creation's reads each task's first event through it, never scanning the events.
#[test]
fn req_arch_004_the_lookups_use_the_index_the_store_has_and_scan_no_table_of_events() {
    let fx = Fx::new();
    let m = made(&fx, Some(request(1)), None);
    let conn = rusqlite::Connection::open_with_flags(
        m._dir.path().join("data/mimlys.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    for (name, sql, bind) in [
        ("of a task", crate::store::EVENT_OF_REQUEST_IN_TASK, 2usize),
        ("of a creation", crate::store::EVENT_OF_REQUEST_CREATING, 1),
    ] {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let params: Vec<String> = (0..bind).map(|_| "x".to_owned()).collect();
        let plan: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params), |r| {
                r.get::<_, String>(3)
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let plan = plan.join(" | ");
        assert!(plan.contains("events_task"), "{name}: {plan}");
        assert!(
            !plan.contains("SCAN e ") && !plan.contains("SCAN events"),
            "{name} scans the events: {plan}"
        );
    }
}

/// A reader that holds no lock finds the same event (REQ-PERS-013): the lookup is a read.
#[test]
fn req_pers_013_a_read_only_reader_finds_the_event_too() {
    let fx = Fx::new();
    let id = request(3);
    let m = made(&fx, Some(id), None);
    let reader = crate::reader::open_read_only(&m._dir.path().join("data")).unwrap();
    let found = reader.event_of_request(None, id).unwrap().expect("found");
    assert_eq!(found.record.request_id, Some(id));
}
