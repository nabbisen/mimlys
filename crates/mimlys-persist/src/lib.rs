//! mimlys persistence: the SQLite store (external design §14.1, §16;
//! REQ-PERS-001 … 012) and, from M3, the content-addressed snapshot store.
//!
//! One database per data directory, WAL with synchronous FULL, owner-only
//! permissions, a lock that refuses a second runtime, forward-only
//! migrations, one transaction per transition with the event appended in
//! it, integrity rules at startup, and the recovery procedure.

pub mod crash;
pub mod integrity;
pub mod metrics;
pub mod recovery;
pub mod snapshots;
pub mod sqlite;
pub mod store;

pub use sqlite::{SCHEMA_VERSION, Store, open};
