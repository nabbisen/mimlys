//! The sweep from outside the sandbox (handoff 011p; audit A-21; external
//! design §13.3, "The tree ends when the worker is killed, on macOS too").
//!
//! The worker's own sweeper is a member of its sandbox, so a tool can signal
//! it and then the worker, and outlive both. The runtime is outside every
//! sandbox and may signal whom it likes, so it sweeps the worker's members
//! itself whenever the worker ends. It cannot use the worker's way of finding
//! members (the pids the profile lets a member inspect); it asks the kernel
//! instead, through the private `sandbox_check` (resolved at run time, never
//! linked, so that a macOS without it is a stated state), whether a process is
//! in a sandbox and what that sandbox allows.
//!
//! **What a member is** (measured by spike M1 on the macOS and Rosetta jobs):
//! a process that is in a sandbox, whose sandbox allows the lookup of the
//! worker's **marker** (a mach service name its profile names and nothing
//! else does), and does **not** allow the lookup of a **canary** (another
//! name nothing names). The canary is what the marker alone lacks: 25 system
//! processes on the runner have a profile that allows every lookup, and the
//! marker alone would have taken them for members. A process that is not in a
//! sandbox allows every lookup, so "sandboxed" comes first.
//!
//! **What the sweep may signal.** Outside the sandbox the kernel no longer
//! refuses a signal to an outsider. A pid that was freed and reused between
//! the look and the signal would be an outsider. The guard: the process's
//! start time is read with the look, and the marker, the canary and the start
//! time are read again immediately before `SIGKILL`; the signal is sent only
//! if all of it still holds. macOS has no process handle, so a gap of about a
//! hundred microseconds remains; measured against 25 s, the shortest time in
//! which a pid came round again (pids are allocated in sequence and wrap at
//! 99999).
//!
//! The decisions are pure and tested on every platform ([`decide`],
//! [`candidates_of`], [`guarded_signal`], [`sweep_outside_with`],
//! [`verification`]); the calls are in `macos/sys.rs`.

use super::{Member, SweepReport, sweep_with};
use std::cell::{Cell, RefCell};
use std::time::Duration;

/// One answer of `sandbox_check`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// "In a sandbox" (the question without a rule), or "allowed" (the question of a lookup).
    Yes,
    /// "Not in a sandbox", or "denied".
    No,
    /// The call failed: nothing is concluded from it.
    Failed,
}

/// Whether a process is a member of the worker's sandbox, from the three
/// questions, asked in this order and no further than needed: in a sandbox,
/// the marker's lookup allowed, the canary's lookup denied. A failed call is
/// never a member.
pub fn decide(
    sandboxed: impl FnOnce() -> Check,
    marker_allowed: impl FnOnce() -> Check,
    canary_allowed: impl FnOnce() -> Check,
) -> bool {
    sandboxed() == Check::Yes && marker_allowed() == Check::Yes && canary_allowed() == Check::No
}

/// When a process started (`proc_pidinfo`, `pbi_start_tvsec`, `pbi_start_tvusec`):
/// with its pid, what a process is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartTime {
    /// Seconds.
    pub sec: u64,
    /// Microseconds.
    pub usec: u64,
}

/// A member the sweep has found and will signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Its process id.
    pub pid: i32,
    /// Its start time, read with the look.
    pub started: StartTime,
}

/// The members among a pid listing: the pids other than `launchd`'s and
/// `exclude`'s whose start time `start_of` reads (a pid it cannot read is
/// gone, or a zombie, which nothing can signal) and `member` takes for one.
pub fn candidates_of(
    listing: &[i32],
    exclude: &[i32],
    mut start_of: impl FnMut(i32) -> Option<StartTime>,
    mut member: impl FnMut(i32) -> bool,
) -> Vec<Candidate> {
    listing
        .iter()
        .copied()
        .filter(|&pid| pid > 1 && !exclude.contains(&pid))
        .filter_map(|pid| {
            let started = start_of(pid)?;
            member(pid).then_some(Candidate { pid, started })
        })
        .collect()
}

/// Why a signal was not sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The process is no longer a member (it ended, or the pid is another process's).
    NotMember,
    /// It is a member, but not the process that was looked at: its start time differs.
    Changed,
}

/// Whether a signal was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// `SIGKILL` was sent.
    Sent,
    /// It was not.
    Skipped(Skip),
}

/// What is read again immediately before the signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recheck {
    /// The marker and the canary say "member" still.
    pub member: bool,
    /// The start time now, when it can be read.
    pub started: Option<StartTime>,
}

/// The reused-pid guard (handoff 011p, item 4): `recheck` is called once, right
/// before the signal, and `send` is called only when the process is still a
/// member **and** started when it did at the first look. The start time is read
/// last in `recheck`, so that nothing but the signal itself lies after it.
pub fn guarded_signal(
    first: StartTime,
    recheck: impl FnOnce() -> Recheck,
    send: impl FnOnce(),
) -> Signal {
    let again = recheck();
    if !again.member {
        return Signal::Skipped(Skip::NotMember);
    }
    if again.started != Some(first) {
        return Signal::Skipped(Skip::Changed);
    }
    send();
    Signal::Sent
}

/// How a sweep from outside ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutsideReport {
    /// The loop's report: signals, rounds and whether the last listing found none.
    pub sweep: SweepReport,
    /// Members the guard did not signal.
    pub skipped: usize,
}

/// The membership kill loop (RFC 011 Amendment 2, point 6: [`sweep_with`]) from
/// outside: list the members, signal each through the guard, repeat until a
/// listing finds none or `grace` passes (LIM-O2).
pub fn sweep_outside_with(
    grace: Duration,
    mut list: impl FnMut() -> Option<Vec<Candidate>>,
    mut signal: impl FnMut(&Candidate) -> Signal,
) -> OutsideReport {
    let current: RefCell<Vec<Candidate>> = RefCell::new(Vec::new());
    let skipped = Cell::new(0usize);
    let sweep = sweep_with(
        grace,
        || {
            let found = list()?;
            let members = found
                .iter()
                .map(|c| Member {
                    pid: c.pid,
                    resident: 0,
                })
                .collect();
            *current.borrow_mut() = found;
            Some(members)
        },
        |pid| {
            let candidate = current.borrow().iter().find(|c| c.pid == pid).copied();
            if let Some(candidate) = candidate
                && let Signal::Skipped(_) = signal(&candidate)
            {
                skipped.set(skipped.get() + 1);
            }
        },
    );
    OutsideReport {
        sweep,
        skipped: skipped.get(),
    }
}

/// What the verification observed (handoff 011p, item 4): the test asked of a
/// known member (the worker just started) and a known outsider (the runtime).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observed {
    /// The known member is found as a member.
    pub member_found: bool,
    /// The runtime itself is found as a member.
    pub outsider_found: bool,
    /// Whether `sandbox_check` says the runtime is in a sandbox.
    pub runtime_sandboxed: Check,
}

/// Why the sweep from outside cannot be relied on, or `Ok` when the test holds
/// on both: the known member is a member and the known outsider is not.
pub fn verification(observed: &Observed) -> Result<(), String> {
    match observed.runtime_sandboxed {
        Check::Yes => return Err("the runtime itself is in a sandbox".to_owned()),
        Check::Failed => {
            return Err("sandbox_check cannot say whether the runtime is in a sandbox".to_owned());
        }
        Check::No => {}
    }
    if !observed.member_found {
        return Err("sandbox_check does not find the worker among its own members".to_owned());
    }
    if observed.outsider_found {
        return Err("sandbox_check takes the runtime for a member of the worker".to_owned());
    }
    Ok(())
}

/// What a test can make wrong in the verification, on a handle it owns: the
/// known member's answer (production never sets it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seam {
    /// The known member's answer is inverted.
    pub wrong_member: bool,
}

/// How many sweeps from outside this process has run for each marker: what the
/// tests read to show that none ran for a worker whose verification failed, or
/// that one did for another.
#[cfg(target_os = "macos")]
static SWEEPS_RUN: std::sync::Mutex<std::collections::BTreeMap<String, usize>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// The sweeps from outside this process has run for the worker of `marker`.
#[cfg(target_os = "macos")]
pub fn sweeps_run(marker: &str) -> usize {
    SWEEPS_RUN
        .lock()
        .map_or(0, |m| m.get(marker).copied().unwrap_or(0))
}

/// `sandbox_check`, resolved.
#[cfg(target_os = "macos")]
pub struct Outside {
    check: super::sys::CheckFn,
}

#[cfg(target_os = "macos")]
impl Outside {
    /// Resolves `sandbox_check` at run time; a macOS that does not have it is an answer.
    pub fn resolve() -> Result<Outside, String> {
        super::sys::resolve_sandbox_check()
            .map(|check| Outside { check })
            .ok_or_else(|| "sandbox_check is not in this macOS".to_owned())
    }

    /// Whether `pid` is in a sandbox.
    pub fn sandboxed(&self, pid: i32) -> Check {
        super::sys::check_sandboxed(self.check, pid)
    }

    /// Whether `pid`'s sandbox allows the lookup of the mach service `name`.
    pub fn allows_lookup(&self, pid: i32, name: &str) -> Check {
        super::sys::check_lookup(self.check, pid, name)
    }

    /// Whether `pid` is a member of the sandbox whose profile names `marker` ([`decide`]).
    pub fn is_member(&self, marker: &str, pid: i32) -> bool {
        decide(
            || self.sandboxed(pid),
            || self.allows_lookup(pid, marker),
            || self.allows_lookup(pid, &super::canary_of(marker)),
        )
    }

    /// The members now, or `None` when the listing fails.
    fn candidates(&self, marker: &str) -> Option<Vec<Candidate>> {
        let listing = super::sys::list_pids().ok()?;
        Some(candidates_of(
            &listing,
            &[std::process::id() as i32],
            super::sys::start_time,
            |pid| self.is_member(marker, pid),
        ))
    }

    /// The sweep: every member of the worker whose profile names `marker`,
    /// signalled through the guard, until none is left or `grace` passes.
    pub fn sweep(&self, marker: &str, grace: Duration) -> OutsideReport {
        if let Ok(mut runs) = SWEEPS_RUN.lock() {
            *runs.entry(marker.to_owned()).or_default() += 1;
        }
        sweep_outside_with(
            grace,
            || self.candidates(marker),
            |c| {
                guarded_signal(
                    c.started,
                    || Recheck {
                        member: self.is_member(marker, c.pid),
                        started: super::sys::start_time(c.pid),
                    },
                    || super::sys::kill_pid(c.pid),
                )
            },
        )
    }
}

/// The sweep of the members of the worker whose profile names `marker`: an
/// error, with nothing signalled, when the marker is not one the runtime
/// makes or `sandbox_check` is not there.
#[cfg(target_os = "macos")]
pub fn sweep(marker: &str, grace: Duration) -> Result<OutsideReport, String> {
    if !super::marker_is_valid(marker) {
        return Err(format!("not a worker marker: {marker:?}"));
    }
    Ok(Outside::resolve()?.sweep(marker, grace))
}

/// The verification of the test on a known member (`worker_pid`, which the
/// runtime has just started) and a known outsider (the runtime itself).
#[cfg(target_os = "macos")]
pub fn verify(worker_pid: i32, marker: &str, seam: Seam) -> Result<(), String> {
    if !super::marker_is_valid(marker) {
        return Err(format!("not a worker marker: {marker:?}"));
    }
    let outside = Outside::resolve()?;
    let own = std::process::id() as i32;
    let found = outside.is_member(marker, worker_pid);
    verification(&Observed {
        member_found: found != seam.wrong_member,
        outsider_found: outside.is_member(marker, own),
        runtime_sandboxed: outside.sandboxed(own),
    })
}

/// The half of the verification that needs no member (the next start, before any
/// worker is alive): `sandbox_check` is there, the runtime is not in a sandbox
/// and is not a member of the marker's.
#[cfg(target_os = "macos")]
pub fn verify_outsider(marker: &str) -> Result<(), String> {
    if !super::marker_is_valid(marker) {
        return Err(format!("not a worker marker: {marker:?}"));
    }
    let outside = Outside::resolve()?;
    let own = std::process::id() as i32;
    verification(&Observed {
        member_found: true,
        outsider_found: outside.is_member(marker, own),
        runtime_sandboxed: outside.sandboxed(own),
    })
}

#[cfg(test)]
mod tests;
