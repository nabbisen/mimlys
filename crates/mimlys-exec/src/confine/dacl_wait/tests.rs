use super::*;
use std::cell::{Cell, RefCell};

/// An attempt that answers from a script and records the timeouts it was given.
fn scripted(script: &[Attempt], seen: &RefCell<Vec<u32>>) -> impl FnMut(u32) -> Attempt {
    let mut answers = script.iter().copied();
    move |timeout| {
        seen.borrow_mut().push(timeout);
        answers.next().expect("no more attempts than scripted")
    }
}

/// REQ-CONF-008: a free lock is taken at once, with no wait and no log line.
#[test]
fn req_conf_008_free_lock_writes_without_waiting() {
    let seen = RefCell::new(vec![]);
    let logged = Cell::new(0);
    let d = decide_with_limit(scripted(&[Attempt::Acquired], &seen), || {
        logged.set(logged.get() + 1)
    });
    assert_eq!(d, Decision::Write);
    assert_eq!(*seen.borrow(), [0]);
    assert_eq!(
        logged.get(),
        0,
        "nothing to say when nothing was waited for"
    );
}

/// REQ-CONF-008: a lock held and released within the wait is written after
/// it; the wait is LIM-O7 and the log line comes once.
#[test]
fn req_conf_008_held_then_released_writes_after_the_wait() {
    let seen = RefCell::new(vec![]);
    let logged = Cell::new(0);
    let d = decide_with_limit(scripted(&[Attempt::Held, Attempt::Acquired], &seen), || {
        logged.set(logged.get() + 1)
    });
    assert_eq!(d, Decision::Write);
    assert_eq!(*seen.borrow(), [0, 60_000], "the wait is LIM-O7");
    assert_eq!(logged.get(), 1, "one line per wait");
}

/// REQ-CONF-008: a lock held past the wait refuses the write, with the log
/// line once; the write is never made.
#[test]
fn req_conf_008_held_past_the_wait_refuses() {
    let seen = RefCell::new(vec![]);
    let logged = Cell::new(0);
    let d = decide_with_limit(scripted(&[Attempt::Held, Attempt::Held], &seen), || {
        logged.set(logged.get() + 1)
    });
    assert_eq!(d, Decision::Refuse(Cause::Held));
    assert_eq!(seen.borrow().len(), 2, "no third attempt, no write");
    assert_eq!(logged.get(), 1);
}

/// REQ-CONF-008: the wait given is the one passed in (LIM-O7 in
/// production), and a shorter one refuses sooner.
#[test]
fn req_conf_008_the_wait_is_the_limit_given() {
    let seen = RefCell::new(vec![]);
    let d = decide(scripted(&[Attempt::Held, Attempt::Held], &seen), || {}, 1);
    assert_eq!(d, Decision::Refuse(Cause::Held));
    assert_eq!(*seen.borrow(), [0, 1]);
    assert_eq!(LIM_O7_PERMISSION_WRITE_WAIT_MS, 60_000);
}

/// REQ-CONF-008: an abandoned lock (its holder died) is acquired and the
/// write proceeds, whether found so at once or after the wait.
#[test]
fn req_conf_008_abandoned_lock_is_acquired() {
    let d = decide_with_limit(
        scripted(&[Attempt::Abandoned], &RefCell::new(vec![])),
        || {},
    );
    assert_eq!(d, Decision::WriteAfterAbandoned);
    let d = decide_with_limit(
        scripted(&[Attempt::Held, Attempt::Abandoned], &RefCell::new(vec![])),
        || {},
    );
    assert_eq!(d, Decision::WriteAfterAbandoned);
}

/// REQ-CONF-008, THR-36: a lock the system will not give (the first call
/// fails, or the wait fails) refuses the write: there is no write without
/// the lock. No log line about waiting, no further attempt.
#[test]
fn req_conf_008_a_lock_the_system_will_not_give_refuses() {
    let seen = RefCell::new(vec![]);
    let logged = Cell::new(0);
    let d = decide_with_limit(scripted(&[Attempt::Failed], &seen), || {
        logged.set(logged.get() + 1)
    });
    assert_eq!(d, Decision::Refuse(Cause::LockUnavailable));
    assert_eq!(*seen.borrow(), [0], "one attempt");
    assert_eq!(logged.get(), 0);
    let d = decide_with_limit(
        scripted(&[Attempt::Held, Attempt::Failed], &RefCell::new(vec![])),
        || {},
    );
    assert_eq!(d, Decision::Refuse(Cause::LockUnavailable));
}

/// REQ-CONF-008: the two refusals' words are external design §13.5's, each
/// naming the directory; the second carries the system's error.
#[test]
fn req_conf_008_refusal_reasons_name_the_directory() {
    assert_eq!(
        Refusal::new(Cause::Held, r"C:\tools\bin", "ignored").to_string(),
        r"acl_not_applied: the permissions of C:\tools\bin are being written by another runtime"
    );
    assert_eq!(
        Refusal::new(
            Cause::LockUnavailable,
            r"C:\tools\bin",
            "Access is denied. (os error 5)"
        )
        .to_string(),
        r"acl_not_applied: the permissions lock of C:\tools\bin cannot be taken: Access is denied. (os error 5)"
    );
}
