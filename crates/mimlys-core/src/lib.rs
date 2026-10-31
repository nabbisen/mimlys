//! mimlys core: task state machine, decision schema, validation, grants,
//! effects, approvals, context assembly, recovery logic, and the adapter SPI.
//!
//! Behaviour is specified in `docs/src/maintainers/requirements.md` and
//! `docs/src/maintainers/external-design.md`. This crate has no I/O and no
//! protocol dependency (REQ-ARCH-002); everything that needs time or
//! identifiers takes a [`clock::Clock`] and an [`ids::IdSource`]
//! (REQ-ARCH-006).
//!
//! Implemented so far: handoff 001 (RFC 001): identifiers, clock, canonical
//! hashing, the task state machine, steps and counters, actions and
//! executions, and the event vocabulary; handoff 002 (RFC 002): the decision
//! schema and strict parser, the tool and schema model, lexical path
//! canonicalization, the validation pipeline, observations, context
//! segments and the adapter SPI; handoff 003 (RFC 003): effect
//! declarations, grants and the policy ceiling, the validator hooks,
//! approvals with runtime rendering, questions, and the completion rule;
//! handoff 004 (RFC 004): the persistence interface and the recovery plan.

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
pub mod observation;
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
