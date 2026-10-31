//! mimlys MCP client adapter (RFC 009): MCP servers as child processes with
//! an explicit environment, discovery and import of their tools under
//! bounds, effects from the administrator's declaration, pinning per task,
//! calls with deadlines and cancellation, results validated against the
//! pinned output schema. The protocol SDK (`rmcp`, pinned to the version
//! rypsok ships with) stays inside this crate (REQ-ARCH-002).
//!
//! Behaviour is specified in `docs/src/maintainers/requirements.md` §21
//! (REQ-MCP) and `docs/src/maintainers/external-design.md` §11.1.

pub mod client;
pub mod import;
pub mod server;
pub mod tools;

#[cfg(feature = "test-server")]
pub mod testserver;

pub use server::{Declaration, ServerConfig};
pub use tools::{McpTools, ServerStatus};
