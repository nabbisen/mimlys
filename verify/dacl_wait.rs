// The Verus proof of the permission-lock decision (RFC 016, REQ-TEST-018).
// This file includes the production module itself; nothing is copied.
use vstd::prelude::*;

#[path = "../crates/mimlys-exec/src/confine/dacl_wait.rs"]
mod dacl_wait;

use dacl_wait::{Attempt, Cause, Decision};

verus! {
#[verifier::external_type_specification]
pub struct ExAttempt(Attempt);
#[verifier::external_type_specification]
pub struct ExCause(Cause);
#[verifier::external_type_specification]
pub struct ExDecision(Decision);
}

fn main() {}
