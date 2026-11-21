//! The machine view (RFC 013 decision 8; external design §19.3; REQ-OBS-009): what the registry
//! says of the user's runtimes, for `status` and `task show`. It reads the registry and changes
//! nothing.
//!
//! The three words keep one meaning here and nowhere else in the interface:
//! - **running**: the runtime holds a slot or an engine gate, which counts against its limit;
//! - **waiting**: the runtime waits for one (its task stays `running` in its own store);
//! - **slot**: one of the `admission_slots` executions of the user across runtimes.

use crate::registry::{Entry, Kind, Registry, Scan, State};
use mimlys_core::clock::Instant;
use serde::Serialize;
use std::path::PathBuf;

/// Whether the registry can be used by a runtime that starts now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Admission {
    /// It can: slots are counted.
    Ready,
    /// It cannot: a runtime goes on unadmitted, which `status` says (the fail-open).
    Unavailable {
        /// Why, in the registry's words.
        reason: String,
    },
}

/// The slots of the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Slots {
    /// `admission_slots` of this configuration (LIM-X5).
    pub limit: usize,
    /// Slots held now, across runtimes.
    pub taken: usize,
}

/// What a runtime waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingFor {
    /// A slot (`admission`).
    Admission,
    /// An engine gate (`engine`).
    Engine,
}

impl WaitingFor {
    fn of(kind: Kind) -> Self {
        match kind {
            Kind::Slot => WaitingFor::Admission,
            Kind::Gate => WaitingFor::Engine,
        }
    }

    /// The word.
    pub fn name(self) -> &'static str {
        match self {
            WaitingFor::Admission => "admission",
            WaitingFor::Engine => "engine",
        }
    }
}

/// What a runtime holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Holds {
    /// An execution slot.
    Slot,
    /// An engine gate.
    EngineGate,
}

/// A runtime that is executing or waiting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeRow {
    /// Its process.
    pub pid: u32,
    /// Its data directory.
    pub data_dir: String,
    /// The task whose execution or request it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// `running` (it holds a slot or a gate) or `waiting`.
    pub state: RowState,
    /// What it holds, when `running`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holds: Option<Holds>,
    /// What it waits for, when `waiting`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<WaitingFor>,
    /// The processes that hold what it waits for, when `waiting`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub behind: Vec<u32>,
    /// The limit it waits against (`admission_slots` or `max_in_flight` of its configuration),
    /// when `waiting`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Since when.
    pub since: Instant,
}

/// The state of a runtime row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowState {
    /// Holds a slot or a gate.
    Running,
    /// Waits for one.
    Waiting,
}

/// An engine endpoint with a gate held or waited for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GateRow {
    /// The endpoint, without credentials or path.
    pub url: String,
    /// Requests in flight now (gates held), across runtimes.
    pub in_flight: usize,
    /// The width in force in the configuration that wrote the entries (`max_in_flight`).
    pub limit: u32,
    /// The processes whose requests are in flight.
    pub holders: Vec<u32>,
    /// The processes that wait for the gate.
    pub waiting: Vec<u32>,
}

/// The machine view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Machine {
    /// The registry's directory, when the platform has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// Whether a runtime starting now would be admitted, or would go on unadmitted.
    pub admission: Admission,
    /// The slots of the user.
    pub slots: Slots,
    /// The runtimes that are executing or waiting.
    pub runtimes: Vec<RuntimeRow>,
    /// The engine gates held or waited for.
    pub engine_gates: Vec<GateRow>,
    /// Entries whose holder is gone, not yet removed: the next runtime that takes a slot removes
    /// them, or `mimlys runtime reap` does.
    pub stale_entries: usize,
}

/// What the registry holds as the view shows it. `limit` is this configuration's
/// `admission_slots`.
pub fn view(dir: Result<PathBuf, String>, limit: usize) -> Machine {
    let empty = |admission, registry| Machine {
        registry,
        admission,
        slots: Slots { limit, taken: 0 },
        runtimes: Vec::new(),
        engine_gates: Vec::new(),
        stale_entries: 0,
    };
    let dir = match dir {
        Ok(dir) => dir,
        Err(reason) => return empty(Admission::Unavailable { reason }, None),
    };
    let shown = dir.to_string_lossy().into_owned();
    let registry = Registry::new(
        dir,
        crate::registry::Bounds::default(),
        std::sync::Arc::new(mimlys_core::clock::SystemClock),
    );
    // Whether a slot could be taken here: the directory can be made private and written.
    let admission = match registry.check() {
        Ok(()) => Admission::Ready,
        Err(reason) => Admission::Unavailable { reason },
    };
    match registry.list() {
        Ok(scan) => Machine {
            registry: Some(shown),
            admission,
            ..from_scan(&scan, limit)
        },
        // The reason the directory cannot be made or written is the better one when there is one.
        Err(reason) => empty(
            match admission {
                Admission::Ready => Admission::Unavailable { reason },
                unavailable => unavailable,
            },
            Some(shown),
        ),
    }
}

/// The view of what a scan found.
pub fn from_scan(scan: &Scan, limit: usize) -> Machine {
    let mut runtimes = Vec::new();
    let mut gates: std::collections::BTreeMap<String, GateRow> = std::collections::BTreeMap::new();
    let holders_of = |entry: &Entry| -> Vec<u32> {
        let mut pids: Vec<u32> = scan
            .live
            .iter()
            .filter(|s| {
                s.entry.state == State::Held
                    && s.entry.kind == entry.kind
                    && s.entry.key == entry.key
            })
            .map(|s| s.entry.pid)
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    };
    let mut taken = 0;
    for seen in &scan.live {
        let entry = &seen.entry;
        let held = entry.state == State::Held;
        if held && entry.kind == Kind::Slot {
            taken += 1;
        }
        if entry.kind == Kind::Gate {
            let row = gates
                .entry(entry.key.clone().unwrap_or_default())
                .or_insert_with(|| GateRow {
                    url: entry.url.clone().unwrap_or_default(),
                    in_flight: 0,
                    limit: entry.limit,
                    holders: Vec::new(),
                    waiting: Vec::new(),
                });
            if held {
                row.in_flight += 1;
                row.holders.push(entry.pid);
            } else {
                row.waiting.push(entry.pid);
            }
        }
        runtimes.push(RuntimeRow {
            pid: entry.pid,
            data_dir: entry.data_dir.clone(),
            task_id: entry.task_id.clone(),
            state: if held {
                RowState::Running
            } else {
                RowState::Waiting
            },
            holds: held.then_some(match entry.kind {
                Kind::Slot => Holds::Slot,
                Kind::Gate => Holds::EngineGate,
            }),
            waiting_for: (!held).then(|| WaitingFor::of(entry.kind)),
            behind: if held { Vec::new() } else { holders_of(entry) },
            limit: (!held).then_some(entry.limit),
            since: entry.since,
        });
    }
    runtimes.sort_by_key(|a| (a.since, a.pid));
    let mut engine_gates: Vec<GateRow> = gates.into_values().collect();
    for gate in &mut engine_gates {
        gate.holders.sort_unstable();
        gate.waiting.sort_unstable();
    }
    Machine {
        registry: None,
        admission: Admission::Ready,
        slots: Slots { limit, taken },
        runtimes,
        engine_gates,
        stale_entries: scan.reaped.len(),
    }
}

/// What a task of this data directory waits for, from the registry (`task show`): the waiting
/// entry of this data directory and task, and who holds what it waits for.
pub fn waiting_of(
    dir: Result<PathBuf, String>,
    data_dir: &str,
    task_id: &str,
) -> Option<mimlys_core::runtime::Waiting> {
    let registry = Registry::new(
        dir.ok()?,
        crate::registry::Bounds::default(),
        std::sync::Arc::new(mimlys_core::clock::SystemClock),
    );
    let scan = registry.list().ok()?;
    let mine = scan.live.iter().find(|s| {
        s.entry.state == State::Waiting
            && s.entry.data_dir == data_dir
            && s.entry.task_id.as_deref() == Some(task_id)
    })?;
    let entry = &mine.entry;
    let mut holders: Vec<u32> = scan
        .live
        .iter()
        .filter(|s| {
            s.entry.state == State::Held && s.entry.kind == entry.kind && s.entry.key == entry.key
        })
        .map(|s| s.entry.pid)
        .collect();
    let taken = holders.len() as u32;
    holders.sort_unstable();
    holders.dedup();
    Some(mimlys_core::runtime::Waiting {
        reason: WaitingFor::of(entry.kind).name().to_owned(),
        limit: entry.limit,
        taken,
        holders,
        since: entry.since,
        url: entry.url.clone(),
    })
}
