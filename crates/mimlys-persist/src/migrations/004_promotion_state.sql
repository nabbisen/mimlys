-- Schema version 4 (handoff 006b): where a promotion is. A promotion is recorded `applying` after
-- its backup is committed and before the first write to the project, `applied` once it is written,
-- and `failed` when it was restored from the backup (external design 14.5, 16.3). The start looks
-- for the ones found `applying`. A promotion recorded before this version was applied.
ALTER TABLE promotions ADD COLUMN state TEXT NOT NULL DEFAULT 'applied';
CREATE INDEX promotions_state ON promotions (state);
