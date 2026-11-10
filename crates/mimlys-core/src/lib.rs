//! mimlys core: task state machine, decision schema, validation, grants,
//! effects, approvals, context assembly, recovery logic, and the adapter SPI.
//!
//! Behaviour is specified in `docs/src/maintainers/requirements.md` and
//! `docs/src/maintainers/external-design.md`. This crate has no I/O and no
//! protocol dependency (REQ-ARCH-002); everything that needs time or
//! identifiers takes a [`clock::Clock`] and an [`ids::IdSource`]
//! (REQ-ARCH-006).
//!
//! What lives here (milestones M1 to M6b): identifiers, the clock, canonical
//! hashing and the task and action state machines (`task`, `action`, `step`,
//! `event`); the decision schema, strict parsing and the twelve-stage
//! validator (`decision`, `schema`, `tool`, `path`, `validate`); effects,
//! grants, the policy ceiling and approvals (`effect`, `grant`, `policy`,
//! `hooks`, `approval`, `question`); observations and context assembly
//! (`observation`, `context`); snapshots as pure data and the recovery plan
//! (`snapshot`, `recovery`); the confinement vocabulary and host-tool
//! declarations (`confine`, `host_tool`); the runtime's step loop and control
//! API (`runtime`); and the adapter SPI (`spi`).

pub mod action;
pub mod approval;
pub mod clock;
pub mod confine;
pub mod context;
pub mod decision;
pub mod effect;
pub mod event;
pub mod grant;
pub mod hash;
pub mod hooks;
pub mod host_tool;
pub mod ids;
pub mod limits;
pub mod native;
pub mod observation;
pub mod panic_text;
pub mod path;
pub mod policy;
pub mod question;
pub mod recovery;
pub mod rejection;
pub mod runtime;
pub mod schema;
pub mod snapshot;
pub mod spi;
pub mod step;
pub mod task;
pub mod tool;
pub mod validate;

#[cfg(test)]
mod tests;
