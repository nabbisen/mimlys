# Handoff 011e — Windows: removing every trace before an uninstall (release 0.8)

**RFC.** [RFC 011](../../done/011-confinement-backends.md) §7 ("record them … so that `revoke` and an uninstall leave the user's profile as it was") and Amendment 1, point 4 · **Specification.** REQ-CONF-008 (amended 2026-11-07), external design §13.5 and §19.2 (`toolchain revoke --all`); threat THR-36 · **Release.** 0.8 · **Depends on** 011b, 011d (done).

## 1. Why

On Windows mimlys changes the machine outside its data directory: it creates an AppContainer profile (`mimlys-<hash>`) and adds access-control entries for that container to mimlys's own directories (applied at every start) and to the toolchain directories the user grants. `toolchain revoke <path>` removes one user entry and refuses mimlys's own. Nothing removes the rest or the profile, so deleting mimlys leaves entries and a profile behind for ever, and if `<data>/toolchain-acls.json` is lost, nothing knows the entries exist. RFC 011 promised otherwise.

## 2. Scope

In:

1. **`mimlys toolchain revoke --all [--yes]`.** `--all` takes no path (a path with `--all` is a usage error, exit 2). On Windows:
   1. Build the list of entries to remove: every entry in the record (user and runtime), **plus** every directory where the container's SID holds an explicit entry although the record does not list it, searched among mimlys's own directories (the workspaces, `<data>/tmp`, the interpreters, the configured modules' directories, the worker binary's directory) and the declared `read_paths` of the configuration. Read "in force" from the directory, as `toolchain list` does, never from the record alone.
   2. Show the list (path, `runtime` or `user`, recorded or found) and the profile name, say that the next start of mimlys applies its own entries again, and ask (one confirmation; `--yes` skips it).
   3. Remove each entry with the existing DACL writer (the named mutex, the explicit-entries merge, the protection flag kept; nothing new in `acl.rs` unless a test shows it is needed), then delete the profile (`DeleteAppContainerProfile`), then the record file.
   4. Report each entry removed and each that failed with the reason; exit 0 only when everything was removed, exit 1 otherwise (the record keeps what failed, so a second run retries exactly that).
   Elsewhere (Linux, macOS): the command says nothing was changed outside the data directory and exits 0, as `grant` and `revoke` do today.
2. **Refusals that stay.** `revoke <path>` keeps refusing mimlys's own directories (they are needed while mimlys is used); `--all` is the one way to remove them, and its confirmation text says that mimlys will apply them again at its next start.
3. **JSON.** `--json` prints `{removed: [{path, source, found_by: record | scan}], failed: [{path, reason}], profile_deleted: bool, record_deleted: bool}`.
4. **Book.** `host-tools.md`, Windows section: a paragraph "Removing mimlys" with the three steps (`mimlys toolchain revoke --all`, `cargo uninstall mimlys`, delete the data directory). `cli.md`: the new form and its JSON. CHANGELOG `Unreleased`, "Added".

Out: changing what `grant`, `revoke <path>` or the startup do; any new access right; Linux or macOS behaviour beyond the message.

## 3. Constraints

- `unsafe` only under `crates/mimlys-exec/src/confine/` (the profile deletion is a `windows-sys` call: one block with its `SAFETY:` line).
- The scan is bounded: only the directories named in item 1.1, never a walk of the disk.
- No new configuration key; no new dependency.
- A partial failure is reported, never hidden; the record is rewritten to hold exactly what was not removed.

## 4. Tests (Windows, `tests/m6b_host_tools.rs`; each names REQ-CONF-008)

| Test | Expected |
|---|---|
| `req_conf_008_revoke_all_leaves_the_directories_as_they_were` | after a start (runtime entries) and `toolchain grant` on two directories: `revoke --all --yes` exits 0; `icacls` output of every touched directory equals its output before mimlys first ran, compared without the `(I)` marker as the M6b suite does; the profile no longer exists (`CreateAppContainerProfile` succeeds afresh, or the profile's folder under `%LOCALAPPDATA%\Packages` is gone); the record file is gone |
| `req_conf_008_revoke_all_finds_entries_without_a_record` | delete the record file after a grant; `revoke --all --yes` still removes the user entry (found by the scan) |
| `req_conf_008_revoke_all_asks_and_reports` | without `--yes` and without a terminal: refused with the message naming `--yes`; `--json` output has the documented shape |
| `req_conf_008_revoke_all_with_a_path_is_a_usage_error` | exit 2 |
| (Linux and macOS, `tests/cli.rs`) `req_conf_008_revoke_all_elsewhere_says_nothing_changed` | exit 0 and the message |

Mutation check (mandatory): remove the scan (record only), run the second test, show it fail.

## 5. Return

Review request `.git-exclude/review-requests/011e-windows-cleanup/README.md`: changed files, the gate, CI run ids per platform with skip counts, the mutation output, differences from this handoff.
