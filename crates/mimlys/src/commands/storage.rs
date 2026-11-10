//! `mimlys status --storage` (REQ-WS-013): what the store holds, by kind, and
//! which tasks hold the most. Read-only: the records through the reader, the
//! files that have no record measured.

use crate::app::{AppError, ReadApp};
use crate::messages;
use crate::output::Output;
use crate::storage;
use mimlys_core::clock::{Clock, SystemClock};

/// Prints the storage view.
pub fn run(app: &ReadApp, out: &Output) -> Result<(), AppError> {
    let view = storage::build(app, SystemClock.now())?;
    for line in messages::storage_lines(&view, storage::STORAGE_TASKS_SHOWN) {
        out.line(line);
    }
    out.value(&view);
    Ok(())
}
