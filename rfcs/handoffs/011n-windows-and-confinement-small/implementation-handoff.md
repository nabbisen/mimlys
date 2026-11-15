# Handoff 011n — Windows and confinement: the small fixes (release 0.11, phase 1)

**RFC.** [RFC 011](../../done/011-confinement-backends.md). **Found by.** The audit before 0.10 (A-23, A-34, A-39), the reviews of 011m, 006b and 003b, and the re-check of 2026-11-16 (`ROADMAP.md`, "Release 0.11", phase 1).

**Specification** (amended before this handoff):
- external design §13.3: the Windows row's mask; the rules "A host tool's argument names no path outside its workspace" and "A task's paths are opened, not compared";
- §13.5: the record of `toolchain-acls.json`.

**No embargo.** These fixes are public as usual. Neutral wording is still good practice; there is no rule against naming what is fixed.

## Items

Each was verified on `main` at `70ab081`; the lines are as found there.

1. **Windows: no `WRITE_DAC`, `WRITE_OWNER` or `GENERIC_ALL` for the container.** `acl.rs:84` grants `GENERIC_ALL` on the workspaces and `<data>/tmp`, so a host tool can change the permissions of any task's files, for example opening them to another local account.
   - Grant read, write, execute and delete only, for example `FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE`. State the exact mask and why each right is needed.
   - **The upgrade:** an existing installation carries the old entry. At start, replace it (one propagation, once) and say so in the log.
   - **`has_entry` compares the mask** (item 2c), or the old entry counts as applied.
2. **A-23, the ACL bookkeeping:**
   - **(a) The lock key.** Key the named mutex on the directory's identity (volume serial and file id, from a handle opened with `FILE_FLAG_BACKUP_SEMANTICS` and `FILE_FLAG_OPEN_REPARSE_POINT`), not on the lowercased text (`acl.rs:186-192`). Write through that handle (`SetSecurityInfo`), so that an alias of the same directory takes the same lock. Look at `profile_name` (`windows.rs:469-474`) for the same alias question, and say what you find.
   - **(b) The record.**
     - An unreadable or corrupt `toolchain-acls.json` **stops the start**, naming the file and the remedy, and is never rewritten (the owner's decision).
     - `save` writes to a temporary file and renames it over the record (`toolchain.rs:89-101`, `:176-178`).
   - **(c) `has_entry`** (`acl.rs:432-463`) accepts an entry only with the wanted mask and not inherited.
   - **(d) The data directory** (`windows.rs:998-1006`) is created with its protected descriptor (`CreateDirectoryW` with security attributes), and a directory whose owner is not the current user is refused.
3. **The permission read retries, under the lock** (the CI flake "os error 122", `GetNamedSecurityInfoW(...mimlys-worker.exe)`).
   - `read_dacl` (`acl.rs:143-168`) retries a few times on `ERROR_INSUFFICIENT_BUFFER`.
   - `prepare` (`toolchain.rs:162`) takes the path's lock for its read too.
4. **Exit code 259.** `try_wait` (`windows.rs:212`) reads exit code 259 (`STILL_ACTIVE`) as still running even after the process handle is signalled. A tool that exits with 259 then hangs the worker, which dies as `worker_died` and spends a restart. After `WAIT_OBJECT_0` the code is final, whatever it is.
5. **A host tool's stdin is closed on Windows.** It is a live pipe today (`windows.rs:808`; `host_tool.rs:409-427`), so a tool that reads stdin waits for its deadline. Drop it at the spawn, as Unix gives `Stdio::null()`.
6. **Handles never leak on an error path, and the attribute list is aligned.**
   - Hold every pipe and handle in an `OwnedHandle` from its creation: `windows.rs:618-620`, `:630-688` and `:772-786`.
   - The attribute list is a `Vec<u8>` (`:627`); allocate it aligned (`Vec<usize>`, or a `u64` buffer).
   - `acl.rs:447` skips `LocalFree` when the count is 0; free whenever the pointer is non-null.
7. **`RLIMIT_AS` is set in the child** (Linux). `host_tool.rs:398-402` and `:625-633` set and restore it in the multi-threaded worker around the spawn, where another thread's allocation can fail. Set it in `pre_exec`, beside `restrict_child`. Check that it is async-signal-safe.
8. **A-34: a host tool's argument names no path outside its workspace.** The rule is in §13.3.
   - Split each argument at `=` and after a short option's letter. Refuse a part that holds a `..` component, or begins with `/`, `\`, `~`, `X:` or `\\`, on every platform.
   - `pattern_allows_paths` (`crates/mimlys-core/src/host_tool.rs:139-143`) goes. A declaration that needs such arguments sets `allow_paths = true`, which `check` and `status` show.
   - **The upgrade:** a declaration that relied on the override now fails `check` with the reason and the remedy (`allow_paths = true`). It must not fail silently at run time.
   - The code comment at `:103-105` then tells the truth.
9. **A task's paths are opened, not compared.** `task_spec` (`confine.rs:68-80`) compares paths lexically: `<data>/workspaces/../tree` passes, and links are not checked.
   - Refuse any component other than a plain name.
   - Open each level without following links: Linux `openat2` with `RESOLVE_NO_SYMLINKS`, or an `O_NOFOLLOW` walk; macOS `O_NOFOLLOW_ANY`; Windows `FILE_FLAG_OPEN_REPARSE_POINT` with a reparse-attribute check.
   - On Linux, give Landlock the opened descriptor.
   - This is the prerequisite of 011o (on Windows a tool can write `<data>/workspaces`).
10. **The runtime's budget for a host tool has a margin** (the CI flake "no termination recorded", `m6b_deadline_kills_the_whole_tree`). The runtime waits `deadline + LIM-O2` from before its request (`worker.rs:669`). The worker starts its own deadline later, after resolution and spawn (`host_tool.rs:470/506`), and may spend up to LIM-O2 in the tree kill.
    - **The budget:** give the runtime a budget that covers the worker's (for example `deadline + 2 × LIM-O2`, or a "deadline reached" frame from the worker). Say which, and why.
    - **The record:** a worker the runtime kills after the tool's deadline has passed is recorded `deadline`, with a note, not `worker_died`.
    - **The log:** log when each frame arrives, so that the next such failure has its timing.

## Tests (by state; each names its requirement)

- **1:** on the Windows job, a host tool calling `SetSecurityInfo` on a file of its task is refused, and the entry's mask is the stated one. **The upgrade:** an old `GENERIC_ALL` entry is replaced at start.
- **2:**
  - (a) two spellings of one directory (a junction or `subst`, and a trailing separator) take one lock;
  - (b) a corrupt record stops the start with the message, the file is unchanged, and a save is atomic. These are pure functions, so they run on every platform;
  - (c) an inherited or wrong-mask entry is not "applied";
  - (d) a data directory owned by another account is refused.
- **3:** two threads granting one file while a third reads, on the Windows job.
- **4:** a tool that exits 259 ends its action `exited` with that code.
- **5:** a tool that reads stdin gets end of input at once.
- **6:** an injected failure at each spawn step leaves no handle open (count them before and after).
- **7:** under load, another thread's allocation never fails during a spawn. The limit still applies to the tool (`alloc` beyond LIM-H2 fails in the tool).
- **8:** every refused form on every platform; `allow_paths = true` admits them; an old declaration fails `check` with the reason.
- **9:** a `..` component and a linked task directory are refused, on every job.
- **10:** a deadline test on the Windows job whose tool's tree kill takes most of LIM-O2 still records `deadline`.

## Mutations

- the old mask;
- `has_entry` without the mask;
- the record rewritten when corrupt;
- 259 read as running;
- stdin left open;
- `RLIMIT_AS` set in the worker again;
- the override kept;
- the lexical `task_spec`;
- the old budget.

## Book and CHANGELOG

- Book:
  - the host-tools page on Windows (the rights a tool has);
  - `allow_paths`;
  - the refusal at start for a corrupt record.
- CHANGELOG: Fixed (each item) and Changed (`allow_paths`, the refusal at start).

Return: `.git-exclude/review-requests/011n-windows-confinement/README.md`.
