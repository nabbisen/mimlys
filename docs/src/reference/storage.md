# Storage

mimlys keeps everything it stores in one directory, the data directory:
`~/.local/share/mimlys` on Linux (`$XDG_DATA_HOME/mimlys`),
`~/Library/Application Support/mimlys` on macOS, `%LOCALAPPDATA%\mimlys` on
Windows, or where `runtime.data_dir` points. Only you can read it. Nothing in
it expires, and nothing is removed unless you ask: a task keeps its copy of
your project until you delete the task.

## What lives where

| Under the data directory | What it is | Grows with |
|---|---|---|
| `workspaces/<task>/tree` | The task's working copy of your project | The project's size, once per task, and what the task writes |
| `workspaces/<task>/tmp` | Scratch space of the task's executions | What a running tool leaves there |
| `snapshots/objects/<hash>` | Every version of every file a task has snapshotted, each content stored once | Each write action adds a snapshot before and after it; identical files, across snapshots and across tasks, share one object |
| `snapshots/manifests/<id>` | The list of files of one snapshot (small) | The number of snapshots |
| `snapshots/pending/<id>` | The note of a snapshot being written; it exists only while the snapshot is written, and the next command that writes finishes or undoes one a crash left | Nothing (empty when no process is writing a snapshot) |
| `blobs/` | The whole text of a stream that a script or a host tool printed, when the excerpt the model reads is a cut of it (the text is cleaned and redacted, as the excerpt is, so no secret is in it): one file per stream, named by the execution's record, holding what the tool printed up to `[sandbox] output_bytes` for a script's stream (1 MiB by default) and 4 MiB for a host tool's (a byte that is not text is stored as three, so a file can be up to three times that). A stream that fits its excerpt has no file. A file whose record a crash never wrote is removed by the next command that writes. [`mimlys task output`](./cli.md) shows it | What a task prints beyond 4 000 characters; it goes with the task (`task delete`, `task prune`) |
| `interpreters/` | Installed interpreters (`mimlys interpreters install`) | Once per language |
| `mimlys.db`, `mimlys.db-wal` | The database: tasks, steps, actions, events | Every event; it is the audit |
| `tmp/` | Temporary files of the worker and of host tools | A running tool |
| `mimlys.lock`, `mimlys.holder` | Who has the store open | Nothing (a few bytes) |

A restore (and the reversal of a promotion) briefly keeps its verified copies in a
directory named `.mimlys-stage-<pid>-<n>` in the root of the tree (or of the project), and removes
it when it ends. `snapshots/objects` and the other directories are not used for it.

Copying the data directory while mimlys is not running is a complete backup.

## Seeing what is stored

<!-- tested -->
```sh
mimlys status --storage
```

shows, without changing anything and while another mimlys process is running:
the data directory, the filesystem it is on and whether that filesystem can
share the blocks of a copy; the bytes by kind (working trees, snapshot objects
that one task uses and that several share, blobs, interpreters, the database
and its log, temporary files); and the tasks that hold the most, with their
state, their age (since they last changed) and what deleting each would free.
`--json` lists every task, not only the first ten.

The sizes of working trees, snapshot objects and blobs are taken from what
mimlys recorded, not from walking the disk, so the command is quick on a large
store. A working tree is counted as of its task's latest snapshot: a file
written into it since is not in the number. Only `tmp/`, the interpreters and
the database files are measured on disk. The sizes are bytes of content, not
disk blocks; the disk usage your system reports for the same directory is
larger by the unused part of each block, and a copy that shares blocks (below)
counts once on disk but fully here.

## Reflinks: how a copy costs nothing

Each task works on a private copy of your project. Where the filesystem can
share blocks between a file and its copy (a reflink), the copy takes no extra
space until a file changes; elsewhere it is a full copy. mimlys asks the
operating system for the copy (`copy_file_range` on Linux, `clonefile` on
macOS, the system's file copy on Windows), so the sharing is the filesystem's
doing. `status --storage` names the filesystem and says whether it can:

| Filesystem | Reflinks | Note |
|---|---|---|
| Btrfs, bcachefs (Linux) | yes | |
| XFS (Linux) | depends | yes when it was made with `reflink=1`, which `mkfs.xfs` does by default since xfsprogs 5.1 |
| ZFS (Linux) | depends | on the version and its settings |
| ext4, tmpfs, f2fs (Linux) | no | |
| APFS (macOS) | yes | |
| HFS+, FAT, exFAT (macOS) | no | |
| ReFS, including a Dev Drive (Windows) | yes | |
| NTFS (Windows) | no | |
| Network filesystems (NFS, SMB) | no | |
| Overlay, FUSE, anything else | unknown | |

`status --storage` shows `yes`, `no` or `unknown` from the filesystem's type; it
does not write a test file to find out. A place that says `no` is not a
problem, only larger: size the disk for one copy of your project per task you
keep.

## What grows, and what does not

- **Working trees** grow with the number of tasks you keep, by the size of the
  project each. A finished task keeps its tree until you delete the task.
- **Snapshot objects** grow with what tasks write. Two tasks that start from the
  same project share the objects they have in common.
- **The database** grows with every event. Deleting a task keeps its events
  (the audit) unless you ask for `--audit`; and the file does not shrink when
  rows go, because the freed space is reused by later rows. mimlys does not
  compact it.
- **Blobs** exist only for a stream that printed more than 4 000 characters, one file
  each, holding at most `[sandbox] output_bytes` of a script's stream (1 MiB by default) and 4 MiB of a host tool's, and up to three times that for bytes that are not text; a task that runs a
  noisy tool again and again holds one for each run. They go with the task, and
  `status --storage` counts them. **Temporary files and interpreters** are small beside the others.

## Reclaiming space

Look first, then choose how much to remove:

<!-- tested -->
```sh
mimlys status --storage
mimlys task prune --older-than 7d
mimlys task prune --state cancelled,failed --yes
mimlys task delete <task>
```

- `mimlys task prune` lists the finished tasks that match (`--state` takes
  `completed`, `failed`, `cancelled`; `--older-than` takes `30s`, `15m`, `2h`,
  `1d`, counted from the task's last change) with what each frees and the
  total, asks once, then deletes them one at a time through the same path as
  `task delete`, saying what each freed. Without a terminal it deletes nothing
  unless you add `--yes`.
- It never touches a task that has not finished, and it keeps a task whose
  promotion you can still reverse, listing it as `kept: promotion reversible`:
  deleting that task would delete the backup `rollback --promotion` needs.
  Reverse the promotion, or delete the task yourself with `task delete`, to
  let it go. A task whose promotion failed and could not restore the project
  from its backup is kept the same way: its backup is the only way back.
- It keeps the audit (the tasks' events) unless you add `--audit`.
- It also lists, with the tasks, each directory of `workspaces` that no task
  refers to, as `no task: left by a refused creation`, with what it holds,
  and removes it under the same confirmation. Releases before 0.11 left such a
  copy of the project when a task was refused after its workspace was made (the
  tools' provider, the store or the clock failed, or the project could not be read); 0.11 removes the workspace
  of a refused creation itself, so these are the remains of an earlier release.
  The filters (`--state`, `--older-than`) do not apply to them; a directory
  whose name is not a task's, and a link, are left alone.
- If one deletion fails, prune stops there, says which task failed and why, and
  names the tasks it did not touch.
- A deletion lists its files in a journal before the rows go and then tries
  every removal (a read-only directory, such as a Go module cache, is made
  writable first). A file that cannot be removed is named by its path, and the
  message says that the task is deleted all the same: `task <id> is deleted; N
  path(s) could not be removed and are left to the next start: …`. The exit
  code is 1, because files remain, and the next command that writes finishes
  the removal. `task prune` counts such a task as deleted, goes on, and finishes
  with exit code 1 and the paths in its report.
- `mimlys task delete <task>` removes one finished task, after showing what
  will go and asking.

## Going back to an older release

A newer mimlys can write words that an older one does not have: a kind of wait, a
reason for a halt, an event. What each release does with the same store:

- **0.13 and later read it.** A command that only reads (`events`, `task show`,
  `task list`, `metrics`, `status`, `task output`, `status --storage`) shows such a
  word as it was written, by its name and with the fields of its record, marked
  `(unknown to this release)`, and goes on: a listing never fails for it, and neither
  does `task show` of the task that holds it. `--json` keeps the record as written
  and adds the key `unknown_to_this_release` to the object that holds the word (see
  [Command line](./cli.md#words-a-newer-release-wrote)).
- **They refuse to act on it.** A command that would change or run such a task
  (`task resume`, `pause`, `cancel`, `delete`, `approve`, `reject`, `answer`,
  `grant`, `revoke`, `rollback`, `promote`, and `--from-task` of a new task) is
  refused with error kind `state`, naming the word and saying that the release of
  mimlys that wrote it is needed, before it asks anything, contacts a backend or
  writes a row. The task is as it was. `task prune` leaves such a task alone and says
  so, and deletes the others.
- **The start leaves it as it is.** A command that writes recovers the tasks it finds
  running and expires what has expired; it does neither for a task that holds such a
  word, and says so once for each task, on standard error. It never halts, rewrites or
  recovers it, and the start does not fail. Run the release that wrote the word to
  go on with the task; nothing is lost by waiting.
- **0.12.0 and earlier fail as before.** They do not read what a later release wrote:
  `events` of 0.11.1 stopped at `storage failure: deserialize` on 0.12's
  `task.admission_changed`, and 0.12.0 does the same at a word of 0.13: the wait kind
  `plan` and the events `plan.approval_requested` and `plan.approval_decided` of a
  plan-first task. A task that waits for its plan makes 0.12.0's `status`, `task show`
  and `events` fail, and every command of 0.12.0 that writes at its start; `events` of
  a task that went through a plan wait fails in 0.12.0 even when it is over; a store
  with no task that waits or waited for a plan is read whole. Before you go back to 0.12.0, approve, reject or
  cancel every plan wait with 0.13. That is a release that is already out, and it is
  not changed. To read a store that a newer
  release wrote, use 0.13 or later; to go on with its tasks, the release that wrote
  them.

A store that holds no such word reads exactly as it did: no marker appears anywhere.
