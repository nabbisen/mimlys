//! The fitness probe (REQ-MDL-011, THR-40; RFC 015 decision 5 and Amendment 1; handoff 015a item
//! 5): `mimlys model probe [<backend>]` runs one scripted mini-task against the real model and
//! says whether the model acts under its transport on this machine.
//!
//! **What it makes.** A scope the runtime creates under `<data_dir>/tmp/probe-<id>/` with one small
//! file ([`FILE`]); a task over it on a runtime of its own (the probe's, built over the same store
//! and the backend probed), under a profile that grants three built-in tools and nothing else
//! ([`profile`]): no promotion, no host tool, no server, no network. The goal is fixed ([`GOAL`]).
//! It is bounded by LIM-E2 steps and LIM-E3 of wall time.
//!
//! **The verdict is the runtime's** ([`classify`]): after the model's `complete` the runtime reads
//! the file in the probe's workspace ([`drive::file_is_as_asked`]); the model's word that it
//! finished is never the verdict.
//!
//! **What it leaves.** Nothing: the task (every record and every event of it, as `task delete
//! --audit` removes them), its workspace and the scope are deleted when the probe ends, whatever
//! the class; a probe that a crash left is found and removed by the next start that writes
//! ([`leftovers`]). It writes no event into a user's task history, and the user's project is never
//! read or written: the only files it reads are the ones it made.
//!
//! **What it records.** One entry per backend in `<data_dir>/probe-results.json`
//! ([`crate::probe_results`]), the latest, written atomically.

pub mod classify;
pub mod drive;
pub mod leftovers;
pub mod run;

use crate::app::AppError;
use crate::config::Config;
use mimlys_core::limits::{LIM_E2_PROBE_STEPS, LIM_E3_PROBE_MS};
use mimlys_core::policy::{OnCancel, OnReject, Policy, Profile};
use mimlys_core::spi::Locality;
use mimlys_core::step::Budgets;

pub use classify::{Class, Detail, End, Observed, StepFacts, Verdict, classify};

/// The name of the probe's profile (the policy of the probe's runtime has no other).
pub const PROFILE: &str = "probe";

/// The tools of the probe's profile: read a file, write a file, replace text in a file. The
/// smallest set that lets a model do the edit either way it knows (all three are built in, run in
/// the runtime's own process, and need no worker).
pub const TOOLS: [&str; 3] = ["read_file", "write_file", "replace_in_file"];

/// The one file of the probe's scope.
pub const FILE: &str = "probe.txt";

/// What the file holds when the probe starts.
pub const FILE_START: &str = "color: red\n";

/// What the file must hold at the end, to a trailing line ending: the verdict's measure.
pub const FILE_WANTED: &str = "color: blue";

/// The probe's goal, fixed: one small edit.
pub const GOAL: &str = "Change the word red to blue in the file probe.txt.";

/// The prefix of the scope's directory name under `<data_dir>/tmp`: `probe-<uuid>`.
pub const SCOPE_PREFIX: &str = "probe-";

/// The most of the probe's file the runtime reads to judge it, in bytes: the file is one line.
pub const READ_AT_MOST: u64 = 4096;

/// The probe's profile. `external` is true only for a backend that is `remote`, named for the
/// probe by the user and allowed by `remote_backend_allowed` ([`plan`]): it is the one place the
/// probe's profile departs from "grants nothing", and the rule of REQ-SEC-010 stays in force at
/// every start of the probe's task (the profile of a local backend says `external = false`, so a
/// backend that turned out remote would halt the task `policy` before anything was sent).
pub fn profile(external: bool) -> Profile {
    Profile {
        name: PROFILE.to_owned(),
        tools: TOOLS.iter().map(|t| (*t).to_owned()).collect(),
        host_tools: Vec::new(),
        budgets: Budgets {
            steps: LIM_E2_PROBE_STEPS,
            ..Budgets::default()
        },
        approval_tools: Vec::new(),
        promote: false,
        external,
        on_reject: OnReject::Continue,
        on_cancel: OnCancel::Rollback,
    }
}

/// The policy of the probe's runtime: the shipped one with the one profile and a ceiling that
/// permits exactly its tools (no host read or write root, no network domain, no host tool).
pub fn policy(external: bool) -> Policy {
    let mut policy = Policy::shipped();
    policy.default_profile = PROFILE.to_owned();
    policy.ceiling.tools = TOOLS.iter().map(|t| (*t).to_owned()).collect();
    policy.ceiling.max_steps = LIM_E2_PROBE_STEPS;
    policy.profiles.clear();
    policy
        .profiles
        .insert(PROFILE.to_owned(), profile(external));
    policy
}

/// The wall time of the probe's task, in milliseconds (LIM-E3): its deadline.
pub const WALL_MS: i64 = LIM_E3_PROBE_MS;

/// A remote backend `model probe` left out, and why.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LeftOut {
    /// The backend's name.
    pub backend: String,
    /// The host of its URL (never the whole URL, which may carry a password); none for a backend
    /// without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// Which backends a `model probe` probes (REQ-MDL-011).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The backends to probe, in order: `default` first, then by name.
    pub probe: Vec<String>,
    /// The remote backends left out, because no name was given.
    pub left_out: Vec<LeftOut>,
    /// The one remote backend that was named and allowed: the command says first that it sends a
    /// fixed mini-task and nothing of the user's.
    pub remote_named: Option<LeftOut>,
}

/// Whether a configured backend is remote.
fn is_remote(section: &crate::config::BackendSection) -> bool {
    section.locality == Locality::Remote
}

/// Chooses what to probe. Without a name: every **local** backend, one after the other, and the
/// remote ones are left out and named. With a name: that backend, and a remote one only when the
/// administrator allowed remote backends (`policy.remote_backend_allowed`); the configuration
/// refuses a remote backend without it already, and this asks again, because it is the probe that
/// sends something to another machine.
pub fn plan(config: &Config, named: Option<&str>) -> Result<Plan, AppError> {
    let host_of = |section: &crate::config::BackendSection| {
        section
            .url
            .as_deref()
            .and_then(|url| crate::backends::host_of(url).ok())
            .map(|host| host.text)
    };
    if let Some(name) = named {
        let section = config.backend(name)?;
        if is_remote(section) {
            if !config.policy.remote_backend_allowed {
                return Err(AppError::Io {
                    detail: crate::messages::probe_remote_not_allowed(name),
                });
            }
            return Ok(Plan {
                probe: vec![name.to_owned()],
                left_out: Vec::new(),
                remote_named: Some(LeftOut {
                    backend: name.to_owned(),
                    host: host_of(section),
                }),
            });
        }
        return Ok(Plan {
            probe: vec![name.to_owned()],
            left_out: Vec::new(),
            remote_named: None,
        });
    }
    let mut names: Vec<&str> = config.backend_names();
    if let Some(at) = names.iter().position(|n| *n == crate::backends::DEFAULT) {
        let default = names.remove(at);
        names.insert(0, default);
    }
    let mut probe = Vec::new();
    let mut left_out = Vec::new();
    for name in names {
        let section = config.backend(name)?;
        if is_remote(section) {
            left_out.push(LeftOut {
                backend: name.to_owned(),
                host: host_of(section),
            });
        } else {
            probe.push(name.to_owned());
        }
    }
    Ok(Plan {
        probe,
        left_out,
        remote_named: None,
    })
}

#[cfg(test)]
mod tests;
