# Handoff 004c — The next start finishes or undoes a pending snapshot journal (release 0.10)

**RFC.** [RFC 004](../../done/004-persistence-checkpoints-and-recovery.md) (recovery), [RFC 006](../../done/006-workspace-snapshots-and-promotion.md) §6 (the journal) · **Specification.** REQ-WS-012 ("a snapshot is a journaled transaction that the next start finishes or undoes, so no orphan is ever left"), external design §16.3 (the last recovery step) and §14.3; REQ-PERS-013 (the read-only open runs no recovery) · **Found.** Handoff 002b item 1, slice 3 finding F1, with its probe.

**What is wrong today.**
- The journal's pieces exist and are unit-tested: `SnapshotFiles::begin`, `finish`, `undo` and `pending`, and `JournalReport`.
- Nothing at start uses them:
  - `App::open` runs `recover_all`, which recovers tasks only, and then the integrity check, which reads the manifests of rows;
  - `LocalWorkspaces::new` only opens the directories.
- So a crash between a journal and its row leaves an object and a manifest that no row and no `task delete` reaches. The probe showed them surviving two writing starts.

1. **At every writing start,** inside the recovery step and before the integrity check, take each pending journal:
   - **its snapshot row exists:** finish it (remove the journal);
   - **no row:** undo it. Remove its manifest, and only those of its listed objects that no snapshot row references.
     - A store written by a release that never recovered may hold a journal whose object a later snapshot reused; that object must stay. Check against the rows, not against the journal alone.
     - Remove the temporary files of an interrupted object write.
2. **Visible.**
   - A start that finished or undid a journal records it in an event: `snapshot.journals_recovered{finished, undone, objects_removed}`. This is an additive event (external design §22); add it to the event list of external design §17.1.
   - Nothing is printed when there was nothing to do.
3. **The read-only open does none of this** (REQ-PERS-013). A pending journal counts as recovery pending: `status` already says `recovery pending: the next command that writes recovers it`, and it says so for a journal too.
4. **An upgrade cleans up.** Journals left by 0.9.0 and earlier are taken by the same step, with the reference check of item 1.

**Tests,** naming REQ-WS-012 (now fully):
- a journal with an object and a manifest and no row: the next writing command removes all three, the integrity check passes, and the event is recorded;
- a journal whose row exists: the journal goes, the objects stay;
- a stale journal listing an object that a later snapshot references: the object stays, and a rollback to that snapshot reads it back;
- the read-only `status` with a pending journal: `recovery pending`, and nothing removed.

**Mutations:**
- the step not called (the first test fails);
- the reference check removed (the third test fails).

Return: `.git-exclude/review-requests/004c-snapshot-journals/README.md`.
