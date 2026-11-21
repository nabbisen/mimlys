//! The registry of the user's runtimes (RFC 013; REQ-SCHED-006, REQ-SCHED-007; THR-37, THR-38),
//! over real files in a temporary directory. Every verdict is a state of the directory or the
//! outcome of a call, never a time: the bounds are small seams, and no test waits for them.

use super::*;
use mimlys_core::clock::FakeClock;
use mimlys_core::ids::TaskId;
use mimlys_core::spi::{AdmissionOutcome, AdmissionRequest, Cancellation, WaitChange};
use std::sync::Mutex;

fn fast() -> Bounds {
    Bounds {
        op: Duration::from_secs(5),
        poll: Duration::from_millis(10),
    }
}

fn registry_in(dir: &Path, bounds: Bounds) -> Arc<Registry> {
    Registry::new(
        dir.to_owned(),
        bounds,
        Arc::new(FakeClock::default()) as Arc<dyn Clock>,
    )
}

fn owner(pid: u32) -> Owner {
    Owner {
        pid,
        data_dir: format!("/data/{pid}"),
    }
}

fn slot(limit: usize) -> Class {
    Class {
        kind: Kind::Slot,
        key: None,
        url: None,
        limit,
    }
}

fn gate_class(url: &str, limit: usize) -> Class {
    let (key, shown) = endpoint_of(url).unwrap();
    Class {
        kind: Kind::Gate,
        key: Some(key),
        url: Some(shown),
        limit,
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "registry.lock")
        .collect();
    names.sort();
    names
}

/// A held entry as a holder that has ended leaves it: the text, and a lock file nobody holds.
fn leave_stale(dir: &Path, pid: u32) -> String {
    let stem = format!("held-slot-dead{pid}");
    let entry = Entry {
        v: ENTRY_VERSION,
        kind: Kind::Slot,
        state: State::Held,
        pid,
        data_dir: "/data/dead".into(),
        task_id: None,
        since: Instant::from_unix_millis(1_700_000_000_000).unwrap(),
        limit: 1,
        key: None,
        url: None,
    };
    std::fs::write(
        dir.join(format!("{stem}.entry")),
        serde_json::to_vec(&entry).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join(format!("{stem}.lock")), b"").unwrap();
    stem
}

async fn granted(registry: &Arc<Registry>, class: &Class, pid: u32) -> Handle {
    match registry
        .take(class, Some("task".into()), &owner(pid), None)
        .await
        .unwrap()
    {
        Take::Granted { handle, .. } => handle,
        Take::Busy { .. } => panic!("expected a free slot"),
    }
}

/// REQ-SCHED-006: at most `limit` slots are held at once; a slot beyond is refused, with the
/// processes that hold the others, and a waiting entry is left; a slot that is returned (its
/// handle dropped) is free for the next.
#[tokio::test]
async fn req_sched_006_a_slot_is_held_until_it_is_returned() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry_in(dir.path(), fast());
    let first = granted(&registry, &slot(1), 1111).await;
    let Take::Busy {
        taken,
        holders,
        waiter,
    } = registry
        .take(&slot(1), Some("t".into()), &owner(2222), None)
        .await
        .unwrap()
    else {
        panic!("the only slot is held");
    };
    assert_eq!((taken, holders), (1, vec![1111]));
    let live = registry.list().unwrap().live;
    let states: Vec<(u32, State)> = live.iter().map(|s| (s.entry.pid, s.entry.state)).collect();
    assert!(states.contains(&(1111, State::Held)), "{states:?}");
    assert!(states.contains(&(2222, State::Waiting)), "{states:?}");
    drop(first);
    // The waiting entry is the waiter's own: it is dropped when the take is granted.
    let Take::Granted { handle, taken } = registry
        .take(&slot(1), Some("t".into()), &owner(2222), Some(waiter))
        .await
        .unwrap()
    else {
        panic!("the slot was returned");
    };
    assert_eq!(taken, 1);
    let live = registry.list().unwrap().live;
    assert_eq!(live.len(), 1, "no waiting entry is left: {live:?}");
    assert_eq!(live[0].entry.state, State::Held);
    drop(handle);
    assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
}

/// REQ-SCHED-006: the count is made under the registry's lock, so runtimes racing for the last
/// slots never hold more than the limit: eight racers for two slots, exactly two are granted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_sched_006_racing_runtimes_never_hold_more_than_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut racers = Vec::new();
    for pid in 0..8u32 {
        // Each racer has a registry of its own over one directory, as a process has.
        let registry = registry_in(dir.path(), fast());
        racers.push(tokio::spawn(async move {
            registry
                .take(&slot(2), None, &owner(100 + pid), None)
                .await
                .unwrap()
        }));
    }
    let mut held = 0;
    let mut kept = Vec::new();
    for racer in racers {
        match racer.await.unwrap() {
            Take::Granted { handle, .. } => {
                held += 1;
                kept.push(handle);
            }
            Take::Busy { .. } => {}
        }
    }
    assert_eq!(held, 2);
}

/// REQ-SCHED-006, THR-38: an entry whose holder is gone (its lock free) is stale: the next take
/// removes it and takes the slot it held, and `reap` lists what it removed; a lock file with
/// no text, and a text with no lock file, are stale too.
#[tokio::test]
async fn req_sched_006_a_stale_entry_is_reaped_at_the_next_take() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry_in(dir.path(), fast());
    let stem = leave_stale(dir.path(), 4242);
    // A reader sees it as stale and removes nothing.
    let scanned = registry.list().unwrap();
    assert!(scanned.live.is_empty());
    assert_eq!(scanned.reaped.len(), 1);
    assert_eq!(scanned.reaped[0].stem, stem);
    assert_eq!(scanned.reaped[0].reason, "its holder is gone");
    assert!(names(dir.path()).contains(&format!("{stem}.entry")));
    // A take removes it and finds the slot free.
    let handle = granted(&registry, &slot(1), 1111).await;
    assert!(!names(dir.path()).contains(&format!("{stem}.entry")));
    drop(handle);
    // `reap` lists it, once.
    let stem = leave_stale(dir.path(), 4343);
    let reaped = registry.reap().unwrap();
    assert_eq!(reaped.len(), 1);
    assert_eq!(reaped[0].stem, stem);
    assert_eq!(reaped[0].entry.as_ref().unwrap().pid, 4343);
    assert!(names(dir.path()).is_empty());
    assert!(registry.reap().unwrap().is_empty());
}

/// THR-38: an entry that cannot be parsed, that names something else than its file name does, or
/// is larger than its bound is refused and removed, and does not count as a slot; the reason
/// is listed.
#[tokio::test]
async fn thr_38_an_entry_that_cannot_be_used_is_refused_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry_in(dir.path(), fast());
    std::fs::write(dir.path().join("held-slot-garbage.entry"), b"{not json").unwrap();
    std::fs::write(dir.path().join("held-slot-huge.entry"), vec![b' '; 5000]).unwrap();
    // A text that says `waiting` under a `held-` name.
    let mut wrong = serde_json::to_value(&Entry {
        v: ENTRY_VERSION,
        kind: Kind::Slot,
        state: State::Waiting,
        pid: 1,
        data_dir: "/d".into(),
        task_id: None,
        since: Instant::from_unix_millis(0).unwrap(),
        limit: 1,
        key: None,
        url: None,
    })
    .unwrap();
    std::fs::write(
        dir.path().join("held-slot-wrong.entry"),
        serde_json::to_vec(&wrong).unwrap(),
    )
    .unwrap();
    // A text with a field it should not have (a crafted entry).
    wrong["extra"] = serde_json::json!(1);
    std::fs::write(
        dir.path().join("waiting-slot-extra.entry"),
        serde_json::to_vec(&wrong).unwrap(),
    )
    .unwrap();
    let before = registry.list().unwrap();
    assert!(before.live.is_empty());
    assert_eq!(before.reaped.len(), 4, "{:?}", before.reaped);
    assert!(
        before
            .reaped
            .iter()
            .all(|r| r.reason.starts_with("it cannot be used")),
        "{:?}",
        before.reaped
    );
    // None of them holds the only slot.
    let handle = granted(&registry, &slot(1), 1).await;
    assert_eq!(
        names(dir.path()),
        [
            format!("{}.entry", handle.stem()),
            format!("{}.lock", handle.stem())
        ]
    );
}

/// THR-38: the registry never follows a link: a symbolic link where an entry's file should be
/// is not read, and is removed as the link it is, leaving what it pointed to as it was.
#[cfg(unix)]
#[tokio::test]
async fn thr_38_a_link_is_never_followed() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("target");
    std::fs::write(&target, b"precious").unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("held-slot-link.entry")).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("held-slot-link.lock")).unwrap();
    let registry = registry_in(dir.path(), fast());
    let listed = registry.list().unwrap();
    assert!(listed.live.is_empty());
    assert_eq!(listed.reaped.len(), 1);
    drop(granted(&registry, &slot(1), 1).await);
    assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
    assert_eq!(std::fs::read(&target).unwrap(), b"precious");
}

/// THR-38: a directory the user does not own alone is not used: it is made private (owner only
/// on Unix) before anything is placed in it.
#[cfg(unix)]
#[tokio::test]
async fn thr_38_the_directory_is_made_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("run");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let registry = registry_in(&dir, fast());
    drop(granted(&registry, &slot(1), 1).await);
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

/// REQ-SCHED-006 (LIM-O11): an operation that cannot get the registry's lock within its bound
/// fails with the reason, instead of waiting for it; so does a directory that is a file, and one
/// that cannot be written.
#[tokio::test]
async fn req_sched_006_a_registry_that_cannot_be_used_fails_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let bounds = Bounds {
        op: Duration::from_millis(100),
        poll: Duration::from_millis(10),
    };
    // The lock is held by someone else (a runtime that stopped in the middle of an operation).
    let registry = registry_in(dir.path(), bounds);
    drop(granted(&registry, &slot(1), 1).await);
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("registry.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let Err(reason) = registry
        .take(&slot(1), None, &owner(2), None)
        .await
        .map(|_| ())
    else {
        panic!("the lock is held");
    };
    assert_eq!(reason, "another runtime holds the registry's lock");
    drop(lock);
    drop(granted(&registry, &slot(1), 1).await);
    // A file where the directory should be.
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"").unwrap();
    let registry = registry_in(&file, fast());
    let Err(reason) = registry
        .take(&slot(1), None, &owner(2), None)
        .await
        .map(|_| ())
    else {
        panic!("a file is not a directory");
    };
    assert!(reason.contains("is not a directory of its own"), "{reason}");
    assert!(registry.check().is_err());
}

/// REQ-SCHED-006: a directory that cannot be written is a registry that cannot be used (Unix
/// permissions; on Windows the test of a file in place of the directory stands for it).
#[cfg(unix)]
#[tokio::test]
async fn req_sched_006_a_directory_that_cannot_be_written_fails_with_a_reason() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("run");
    let registry = registry_in(&dir, fast());
    drop(granted(&registry, &slot(1), 1).await);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    // A process with the right to write anywhere (root) is not refused: nothing to prove.
    if std::fs::File::create(dir.join("probe")).is_ok() {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let Err(reason) = registry
        .take(&slot(1), None, &owner(2), None)
        .await
        .map(|_| ())
    else {
        panic!("the directory cannot be written");
    };
    assert!(reason.contains("creating"), "{reason}");
    // The directory is the user's own: the next process makes it private again, and it works.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(registry.check().is_ok());
    drop(granted(&registry, &slot(1), 1).await);
}

/// REQ-SCHED-007: a gate is per endpoint: two URLs of one engine share it (whatever the path,
/// the case of the host or the credentials), two engines do not, and a gate does not count
/// against the slots.
#[tokio::test]
async fn req_sched_007_a_gate_is_per_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry_in(dir.path(), fast());
    let a = gate_class("http://Localhost:8080/v1", 1);
    let same = gate_class("http://user:secret@localhost:8080/other?x=1", 1);
    let other = gate_class("http://localhost:8081/v1", 1);
    assert_eq!(a, same);
    assert_ne!(a.key, other.key);
    assert_eq!(a.url.as_deref(), Some("http://localhost:8080"));
    let held = granted(&registry, &a, 1).await;
    assert!(matches!(
        registry.take(&same, None, &owner(2), None).await.unwrap(),
        Take::Busy { .. }
    ));
    // Another engine, and the slots, are free.
    drop(granted(&registry, &other, 2).await);
    drop(granted(&registry, &slot(1), 2).await);
    // A gate of width two admits two.
    let wide = gate_class("http://localhost:9000", 2);
    let (one, two) = (
        granted(&registry, &wide, 1).await,
        granted(&registry, &wide, 2).await,
    );
    assert!(matches!(
        registry.take(&wide, None, &owner(3), None).await.unwrap(),
        Take::Busy { .. }
    ));
    drop((held, one, two));
    assert_eq!(endpoint_of("not a url"), None);
}

/// REQ-OBS-009: a reader sees every entry (a held slot, a held gate, a waiting runtime) with what
/// the entry says, and changes nothing; a directory that is not there holds nothing.
#[tokio::test]
async fn req_obs_009_a_reader_lists_the_entries_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry_in(&dir.path().join("absent"), fast());
    assert_eq!(registry.list().unwrap(), Scan::default());
    let registry = registry_in(dir.path(), fast());
    let _slot = granted(&registry, &slot(1), 10).await;
    let _gate = granted(&registry, &gate_class("http://localhost:1234", 1), 11).await;
    let waiting = registry
        .take(&slot(1), Some("tw".into()), &owner(12), None)
        .await
        .unwrap();
    assert!(matches!(waiting, Take::Busy { .. }));
    let before = names(dir.path());
    let mut live: Vec<(u32, Kind, State, Option<String>)> = registry
        .list()
        .unwrap()
        .live
        .iter()
        .map(|s| {
            (
                s.entry.pid,
                s.entry.kind,
                s.entry.state,
                s.entry.task_id.clone(),
            )
        })
        .collect();
    live.sort();
    assert_eq!(
        live,
        [
            (10, Kind::Slot, State::Held, Some("task".into())).clone(),
            (11, Kind::Gate, State::Held, Some("task".into())),
            (12, Kind::Slot, State::Waiting, Some("tw".into())),
        ]
        .map(|(pid, kind, state, task)| (pid, kind, state, task))
    );
    assert_eq!(names(dir.path()), before);
    drop(waiting);
}

type Changes = Arc<Mutex<Vec<WaitChange>>>;

fn recorder(
    changes: &Changes,
) -> impl Fn(WaitChange) -> Result<(), mimlys_core::spi::PersistenceError> + Send + Sync + use<> {
    let changes = changes.clone();
    move |change| {
        changes.lock().unwrap().push(change);
        Ok(())
    }
}

fn request(remaining_ms: i64) -> AdmissionRequest {
    AdmissionRequest {
        task_id: TaskId::from_uuid(uuid::Uuid::from_u128(7)),
        remaining_ms,
    }
}

/// Waits (by looking at the directory, not the clock) until `count` runtimes wait.
async fn until_waiting(registry: &Arc<Registry>, count: usize) {
    let started = std::time::Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "{count} runtime(s) never waited"
        );
        let waiting = registry
            .list()
            .unwrap()
            .live
            .iter()
            .filter(|s| s.entry.state == State::Waiting)
            .count();
        if waiting >= count {
            return;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// REQ-SCHED-006: a wait for a slot reports `Waiting` once with the slots taken and the processes
/// that hold them, leaves a waiting entry that names the task, and is admitted when the holder
/// returns its slot, reporting `Admitted` with how long it waited; the waiting entry is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_sched_006_a_wait_ends_when_the_holder_returns_its_slot() {
    let dir = tempfile::tempdir().unwrap();
    let first = registry_in(dir.path(), fast());
    let second = registry_in(dir.path(), fast());
    let holder = slots(first.clone(), 1, owner(1111));
    let waiter = slots(second.clone(), 1, owner(2222));
    let changes = Changes::default();
    let record = recorder(&changes);
    let AdmissionOutcome::Admitted {
        held: Some(held), ..
    } = holder
        .admit(request(60_000), Cancellation::new(), &|_| Ok(()))
        .await
        .unwrap()
    else {
        panic!("the first runtime takes the slot");
    };
    let waiting = {
        let changes = changes.clone();
        let record = move |c| {
            let _ = &changes;
            record(c)
        };
        let waiter = waiter.clone();
        tokio::spawn(async move {
            waiter
                .admit(request(60_000), Cancellation::new(), &record)
                .await
        })
    };
    until_waiting(&first, 1).await;
    let entries = first.list().unwrap().live;
    let waits: Vec<&Seen> = entries
        .iter()
        .filter(|s| s.entry.state == State::Waiting)
        .collect();
    assert_eq!(waits[0].entry.pid, 2222);
    assert_eq!(waits[0].entry.data_dir, "/data/2222");
    assert_eq!(
        waits[0].entry.task_id.as_deref(),
        Some(request(1).task_id.to_string().as_str())
    );
    assert_eq!(
        *changes.lock().unwrap(),
        [WaitChange::Waiting {
            slots_taken: 1,
            holders: vec![1111]
        }]
    );
    drop(held);
    let outcome = waiting.await.unwrap().unwrap();
    let AdmissionOutcome::Admitted { held, .. } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(held.is_some());
    let changes = changes.lock().unwrap().clone();
    assert_eq!(changes.len(), 2);
    assert!(matches!(
        changes[1],
        WaitChange::Admitted { slots_taken: 1, .. }
    ));
    let live = first.list().unwrap().live;
    assert!(
        live.iter().all(|s| s.entry.state == State::Held),
        "no waiting entry is left: {live:?}"
    );
}

/// REQ-SCHED-006, RFC 013 Amendment 1: the first stop request ends the wait with nothing held and
/// no entry left, and a request already made takes no slot at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_sched_006_the_stop_request_ends_the_wait_and_leaves_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let first = registry_in(dir.path(), fast());
    let second = registry_in(dir.path(), fast());
    let holder = slots(first.clone(), 1, owner(1111));
    let waiter = slots(second.clone(), 1, owner(2222));
    let AdmissionOutcome::Admitted { held, .. } = holder
        .admit(request(60_000), Cancellation::new(), &|_| Ok(()))
        .await
        .unwrap()
    else {
        panic!("a free slot");
    };
    let stop = Cancellation::new();
    let waiting = {
        let (waiter, stop) = (waiter.clone(), stop.clone());
        tokio::spawn(async move { waiter.admit(request(60_000), stop, &|_| Ok(())).await })
    };
    until_waiting(&first, 1).await;
    stop.cancel();
    let outcome = waiting.await.unwrap().unwrap();
    assert!(
        matches!(outcome, AdmissionOutcome::Stopped { .. }),
        "{outcome:?}"
    );
    let live = first.list().unwrap().live;
    assert_eq!(live.len(), 1, "only the holder's entry: {live:?}");
    // A request made before the call takes nothing, not even a free slot.
    drop(held);
    let outcome = waiter
        .admit(request(60_000), stop.clone(), &|_| Ok(()))
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        AdmissionOutcome::Stopped { waited_ms: 0 }
    ));
    assert!(first.list().unwrap().live.is_empty());
}

/// REQ-SCHED-006: the task's deadline ends the wait, with nothing held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_sched_006_the_deadline_ends_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let first = registry_in(dir.path(), fast());
    let holder = slots(first.clone(), 1, owner(1111));
    let _held = holder
        .admit(request(60_000), Cancellation::new(), &|_| Ok(()))
        .await
        .unwrap();
    let waiter = slots(registry_in(dir.path(), fast()), 1, owner(2222));
    let outcome = waiter
        .admit(request(30), Cancellation::new(), &|_| Ok(()))
        .await
        .unwrap();
    assert!(
        matches!(outcome, AdmissionOutcome::DeadlinePassed { .. }),
        "{outcome:?}"
    );
    assert_eq!(first.list().unwrap().live.len(), 1);
}

/// REQ-SCHED-006, THR-37: the fail-open. A registry that cannot be used lets the execution go on
/// with nothing held, and reports why (`Unadmitted`), for a directory that is a file and for no
/// directory at all.
#[tokio::test]
async fn req_sched_006_a_registry_that_cannot_be_used_lets_the_execution_go_on_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a-file");
    std::fs::write(&file, b"").unwrap();
    for admission in [
        slots(registry_in(&file, fast()), 1, owner(1)),
        slots_in(
            Err("no home directory is known for this user".into()),
            1,
            owner(1),
        ),
    ] {
        let changes = Changes::default();
        let outcome = admission
            .admit(request(60_000), Cancellation::new(), &recorder(&changes))
            .await
            .unwrap();
        assert!(
            matches!(outcome, AdmissionOutcome::Admitted { held: None, .. }),
            "{outcome:?}"
        );
        let changes = changes.lock().unwrap().clone();
        assert!(
            matches!(&changes[..], [WaitChange::Unadmitted { reason }] if !reason.is_empty()),
            "{changes:?}"
        );
    }
}

/// THR-38: a holder that ends without returning its slot (killed) frees it: a waiter is admitted
/// once the entry's lock is gone, which the next look finds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thr_38_a_slot_whose_holder_ended_is_free_for_the_waiter() {
    let dir = tempfile::tempdir().unwrap();
    let stem = leave_stale(dir.path(), 9999);
    let waiter = slots(registry_in(dir.path(), fast()), 1, owner(2222));
    let outcome = waiter
        .admit(request(60_000), Cancellation::new(), &|_| Ok(()))
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        AdmissionOutcome::Admitted { held: Some(_), .. }
    ));
    assert!(!names(dir.path()).contains(&format!("{stem}.entry")));
}
