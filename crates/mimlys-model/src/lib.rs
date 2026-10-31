//! mimlys model backends: the fake backend used in tests and replay (M1) and
//! the OpenAI-compatible HTTP adapter (M4, RFC 008).
//!
//! Behaviour is specified in `docs/src/maintainers/requirements.md` §19.

pub mod fake;
pub mod openai;

#[cfg(feature = "mock-server")]
pub mod mock;
