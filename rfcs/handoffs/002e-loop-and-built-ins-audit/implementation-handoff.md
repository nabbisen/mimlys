# Handoff 002e — A failed restore is never silent; recovery survives a crash; a built-in never blocks; the specification's effect rule (release 0.10)

**RFC.** [RFC 002](../../done/002-decision-schema-and-validation-order.md) (validation and built-in tools) and RFC 004 (recovery) · **Found by.** The independent audit before 0.10, which audited commit `f5a94fa`; X-04 and X-05 come from the architect's triage. The owner accepted the triage on 2026-11-14.

**Specification** (amended before this handoff):
- external design §16.3: a task found in `recovering`;
- §6.3 and §6.4: no separate effect rule of the profile, and no `effect_not_allowed`.

**The defects, verified in the code at `f5a94fa`:**
- **A-17.** `runtime/loop_.rs:1284` throws a restore error away (`Err(_) => unknown_after_restore = true`, no log). For a `Cancelled` execution, `cause` is `None` (`:1243`), so the function returns at `:1374` before `:1381` reads the flag.
  - With `on_cancel = rollback` (the default, `policy.rs:35`), a cancelled workspace write whose restore failed is recorded `cancelled` with `rolled_back = false` and no notice.
  - The task pauses, and a resume continues on a half-restored tree.
- **A-18.** Startup recovery scans only `running` (`mimlys-persist/src/recovery.rs:76`), and `plan_recovery` returns `None` unless the task is `Running` (`mimlys-core/src/recovery.rs:46-48`). Its steps commit one by one (`:86-152`).
  - A crash, or a Ctrl-C, during a long restore leaves the task `recovering` for good. Only `cancel` gets it out.
- **X-04.** `builtin/read_file.rs` reads the path whole (`body.dir.read`) in a blocking task (`builtin.rs:181`), with no file-type check.
  - On a FIFO with no writer it blocks for good. The deadline ends the action, but the thread keeps the runtime from exiting.
  - The unreported run's `mimlys run` was still alive 100 s after the task completed.
  - The other built-ins that open files share the question.
- **X-05.** An action that makes a directory unreadable (`chmod 000 d`) makes the next snapshot's walk fail with `workspace storage failure: walk`. That is an internal error which leaves the task `running`. Rollback and resume fail the same way; only `cancel` works.
  - This was shown by the unreported run's evidence; I have not re-read it in the code.
- **A-35.** `DeniedReason::EffectNotAllowed` is defined, hinted and documented, and produced nowhere. The doc comment at `validate.rs:26` claims "the profile's effect rules", which do not exist.

## Items

1. **A failed restore is never silent** (A-17).
   - Read the restore's result before the early return for a `cancelled` execution.
   - Log the error at `warn`, with the path.
   - Halt `unknown_outcome`, as a failed restore after a failure already does. The observation says that it was not rolled back.
2. **Recovery survives its own crash** (A-18).
   - The scan includes `recovering`, and `plan_recovery` plans for it.
   - Every step is idempotent: marking the action `unknown` again, restoring again, and recording `Recovered` once.
   - The restore comes before the action is marked.
3. **A built-in never blocks on a file** (X-04).
   - Every built-in that opens a path opens it without blocking (`O_NONBLOCK` with no follow, then `fstat`).
   - Anything that is not a regular file (or, for the directory tools, a directory) is refused: `not_a_file`, or the existing word that fits, with its hint.
   - The read stops at the bytes the tool can use, not the whole file. State the bound you take.
4. **An unreadable tree is a stated halt, never an internal error** (X-05).
   - A snapshot, rollback or fork that cannot read part of the tree ends the action with a stated cause, and halts the task with a reason and a remedy line naming the path, for example `restore its permissions, then resume`.
   - It never leaves the task `running` with an action that will never finish.
   - Propose the halt reason, an existing one if one fits; I register a new one at review.
   - The walk's other gaps (A-38: non-UTF-8 names, depth, FIFOs) are 0.11's. Do not widen this item.
5. **The specification's effect rule** (A-35).
   - Remove `DeniedReason::EffectNotAllowed`, its hint and its name, and the claim at `validate.rs:26`.
   - The book's decisions page drops `effect_not_allowed`.
   - CHANGELOG: Changed, for library users.

## Tests (by state, §3c)

- **1:** a cancelled workspace write whose restore fails (an injected failure): the task is halted `unknown_outcome`, the observation is not rolled back, and the log line is present. REQ-WS-010.
- **2:** a crash injected after `StartRecovery`, and again after the mark: the next start completes the recovery, and the task leaves `recovering`. REQ-PERS-007.
- **3:** `read_file` on a FIFO with no writer: refused at once, and the runtime exits within LIM-O1. The same holds for a socket, and for the other built-ins that open paths.
- **4:** `chmod 000 d` by a host tool: the task halts with the reason and the remedy; after `chmod 755` and `resume`, it continues.
- **5:** the vocabulary test lists no `effect_not_allowed`.

## Mutations

- the early return before the restore's result again;
- `recovering` not scanned;
- a blocking open;
- the walk's error kept internal.

Return: `.git-exclude/review-requests/002e-loop-audit/README.md`.
