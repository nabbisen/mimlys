//! mimlys execution: built-in tools over capability handles, the worker
//! protocol and the worker's entry point, the Wasmtime sandbox, the
//! confinement interface with its platform backends, and (M6) host tools.
//!
//! Behaviour is specified in `docs/src/maintainers/requirements.md` and
//! `docs/src/maintainers/external-design.md`.

pub mod builtin;
pub mod confine;
pub mod host_tool;
pub mod interpreters;
pub mod protocol;
pub mod sandbox;
pub mod tools;
pub mod trace;
pub mod worker;
pub mod worker_main;
