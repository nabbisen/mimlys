//! Unit tests of the sweep from outside the sandbox: the decisions are pure and
//! run on every platform (handoff 011p; spike M1 measured what the kernel
//! answers; the macOS job's tests run it against real sandboxes).

use super::*;
use std::cell::{Cell, RefCell};

fn at(sec: u64) -> StartTime {
    StartTime { sec, usec: 5 }
}

/// Handoff 011p, item 1: a member is in a sandbox, allows the marker and does not
/// allow the canary; each of the three alone, or any two, is not enough, and a
/// failed call is never a member. The spike found 25 system processes that
/// allow every lookup (marker yes, canary yes), and every unsandboxed process
/// allows every lookup too (sandboxed no).
#[test]
fn req_conf_009_a_member_is_sandboxed_allows_the_marker_and_not_the_canary() {
    use Check::{Failed, No, Yes};
    for sandboxed in [Yes, No, Failed] {
        for marker in [Yes, No, Failed] {
            for canary in [Yes, No, Failed] {
                let expected = sandboxed == Yes && marker == Yes && canary == No;
                assert_eq!(
                    decide(|| sandboxed, || marker, || canary),
                    expected,
                    "sandboxed {sandboxed:?}, marker {marker:?}, canary {canary:?}"
                );
            }
        }
    }
    // The permissive system process: the marker and the canary both allowed.
    assert!(!decide(|| Yes, || Yes, || Yes));
    // An unsandboxed process (the user's own): everything is allowed.
    assert!(!decide(|| No, || Yes, || Yes));
}

/// Handoff 011p, item 1: the questions are asked in order and no further than
/// needed: most processes of a machine are not this worker's, and each call costs.
#[test]
fn req_conf_009_the_questions_stop_at_the_first_no() {
    let asked = RefCell::new(Vec::new());
    let ask = |what: &'static str, answer: Check| {
        let asked = &asked;
        move || {
            asked.borrow_mut().push(what);
            answer
        }
    };
    assert!(!decide(
        ask("sandboxed", Check::No),
        ask("marker", Check::Yes),
        ask("canary", Check::No)
    ));
    assert_eq!(*asked.borrow(), vec!["sandboxed"]);
    asked.borrow_mut().clear();
    assert!(!decide(
        ask("sandboxed", Check::Yes),
        ask("marker", Check::No),
        ask("canary", Check::No)
    ));
    assert_eq!(*asked.borrow(), vec!["sandboxed", "marker"]);
    asked.borrow_mut().clear();
    assert!(decide(
        ask("sandboxed", Check::Yes),
        ask("marker", Check::Yes),
        ask("canary", Check::No)
    ));
    assert_eq!(*asked.borrow(), vec!["sandboxed", "marker", "canary"]);
}

/// Handoff 011p, item 2: the listing is cut down to the members: `launchd`
/// and below, the runtime, a pid whose start time cannot be read (gone, or a
/// zombie, which nothing can signal) and every non-member are out.
#[test]
fn req_conf_009_the_candidates_are_the_members_that_can_be_signalled() {
    let listing = [0, 1, 2, 10, 11, 12, 13, 14];
    let found = candidates_of(
        &listing,
        &[14],
        |pid| (pid != 12).then(|| at(pid as u64)),
        |pid| matches!(pid, 2 | 10 | 12 | 14),
    );
    assert_eq!(
        found,
        vec![
            Candidate {
                pid: 2,
                started: at(2)
            },
            Candidate {
                pid: 10,
                started: at(10)
            },
        ],
        "2 is a member; 10 is; 12 has no readable start; 14 is excluded; 11 and 13 are not members"
    );
}

/// Handoff 011p, item 4: the guard. A pid that is a member still but started at
/// another time is another process (a reused pid): nothing is sent. The
/// mutation: the start time not compared.
#[test]
fn req_conf_009_a_reused_pid_is_not_signalled() {
    let sent = Cell::new(0);
    // The seam: between the look and the signal the pid was freed and reused by
    // a process that is a member as well (the worst case: the marker and the
    // canary cannot tell), started later.
    let reused = guarded_signal(
        at(100),
        || Recheck {
            member: true,
            started: Some(at(101)),
        },
        || sent.set(sent.get() + 1),
    );
    assert_eq!(reused, Signal::Skipped(Skip::Changed));
    assert_eq!(sent.get(), 0, "nothing was signalled");
    // The start time cannot be read any more: the process is gone.
    let gone = guarded_signal(
        at(100),
        || Recheck {
            member: true,
            started: None,
        },
        || sent.set(sent.get() + 1),
    );
    assert_eq!(gone, Signal::Skipped(Skip::Changed));
    assert_eq!(sent.get(), 0);
}

/// Handoff 011p, item 4: a pid that is not a member now (an outsider took it,
/// or the process left the sandbox's test) is not signalled, even when the
/// start time reads the same.
#[test]
fn req_conf_009_a_process_that_is_no_longer_a_member_is_not_signalled() {
    let sent = Cell::new(0);
    let r = guarded_signal(
        at(100),
        || Recheck {
            member: false,
            started: Some(at(100)),
        },
        || sent.set(sent.get() + 1),
    );
    assert_eq!(r, Signal::Skipped(Skip::NotMember));
    assert_eq!(sent.get(), 0);
}

/// Handoff 011p, item 4: the same process, still a member, is signalled once,
/// and the recheck is made once, before the signal.
#[test]
fn req_conf_009_the_same_member_is_signalled_once_after_one_recheck() {
    let order = RefCell::new(Vec::new());
    let r = guarded_signal(
        at(100),
        || {
            order.borrow_mut().push("recheck");
            Recheck {
                member: true,
                started: Some(at(100)),
            }
        },
        || order.borrow_mut().push("signal"),
    );
    assert_eq!(r, Signal::Sent);
    assert_eq!(*order.borrow(), vec!["recheck", "signal"]);
}

/// A table of processes the loop sweeps: `lag` signals are needed before one ends.
struct World {
    alive: Vec<(i32, StartTime)>,
    signals: Vec<i32>,
}

/// Handoff 011p, item 2: the sweep repeats until a listing finds no member, and
/// counts what the guard skipped; it reports complete only on an empty listing.
#[test]
fn req_conf_009_the_outside_sweep_ends_when_no_member_is_listed() {
    let world = RefCell::new(World {
        alive: vec![(10, at(1)), (11, at(2)), (12, at(3))],
        signals: vec![],
    });
    // Member 11's pid is reused by another process between the look and the signal:
    // its start time is not the one that was seen at the first look (the first time only).
    let seen_11 = Cell::new(false);
    let report = sweep_outside_with(
        Duration::from_secs(60),
        || {
            Some(
                world
                    .borrow()
                    .alive
                    .iter()
                    .map(|(pid, started)| Candidate {
                        pid: *pid,
                        started: *started,
                    })
                    .collect(),
            )
        },
        |c| {
            let reused = c.pid == 11 && !seen_11.replace(true);
            guarded_signal(
                c.started,
                || Recheck {
                    member: true,
                    started: Some(if reused { at(99) } else { c.started }),
                },
                || {
                    let mut w = world.borrow_mut();
                    w.signals.push(c.pid);
                    w.alive.retain(|(pid, _)| *pid != c.pid);
                },
            )
        },
    );
    assert!(report.sweep.complete, "{report:?}");
    assert_eq!(
        report.skipped, 1,
        "the reused pid was skipped once: {report:?}"
    );
    assert!(world.borrow().alive.is_empty());
    assert_eq!(
        world.borrow().signals,
        vec![10, 12, 11],
        "11 on the second round"
    );
    assert!(report.sweep.rounds >= 2, "{report:?}");
}

/// Handoff 011p, item 2: a member that is never gone, or a listing that fails,
/// is incomplete (`tree_kill_incomplete`), never complete on faith.
#[test]
fn req_conf_009_the_outside_sweep_reports_what_it_could_not_do() {
    let stubborn = sweep_outside_with(
        Duration::from_millis(60),
        || {
            Some(vec![Candidate {
                pid: 10,
                started: at(1),
            }])
        },
        |_| Signal::Sent,
    );
    assert!(
        !stubborn.sweep.complete && stubborn.sweep.rounds >= 1,
        "{stubborn:?}"
    );
    let blind = sweep_outside_with(Duration::from_secs(1), || None, |_| panic!("no signal"));
    assert!(!blind.sweep.complete);
    assert_eq!(blind.sweep.killed, 0);
}

/// Handoff 011p, item 4: the verification. The known member must be found and
/// the known outsider (the runtime) must not, and the runtime must not be in a
/// sandbox itself; each failure says which. The mutation: the verification skipped.
#[test]
fn req_conf_009_the_verification_needs_the_member_found_and_the_outsider_not() {
    let good = Observed {
        member_found: true,
        outsider_found: false,
        runtime_sandboxed: Check::No,
    };
    assert_eq!(verification(&good), Ok(()));
    let no_member = verification(&Observed {
        member_found: false,
        ..good
    })
    .unwrap_err();
    assert!(
        no_member.contains("does not find the worker"),
        "{no_member}"
    );
    let outsider = verification(&Observed {
        outsider_found: true,
        ..good
    })
    .unwrap_err();
    assert!(
        outsider.contains("takes the runtime for a member"),
        "{outsider}"
    );
    let sandboxed = verification(&Observed {
        runtime_sandboxed: Check::Yes,
        ..good
    })
    .unwrap_err();
    assert!(sandboxed.contains("in a sandbox"), "{sandboxed}");
    let failed = verification(&Observed {
        runtime_sandboxed: Check::Failed,
        ..good
    })
    .unwrap_err();
    assert!(failed.contains("cannot say"), "{failed}");
}
