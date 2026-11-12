-- Schema version 3 (handoff 002c): the typed detail of a rejection, as the
-- model read it (REQ-DEC-008, REQ-DEC-009), as JSON beside the step's code.
-- Null for a step that was not rejected and for a step recorded before this
-- version, whose feedback is the code's hint.
ALTER TABLE steps ADD COLUMN rejection_detail TEXT;
