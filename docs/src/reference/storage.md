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
| `blobs/<id>` | Captured outputs of tools (at most 1 MiB each) | The output of tool runs |
| `interpreters/` | Installed interpreters (`mimlys interpreters install`) | Once per language |
| `mimlys.db`, `mimlys.db-wal` | The database: tasks, steps, actions, events | Every event; it is the audit |
| `tmp/` | Temporary files of the worker and of host tools | A running tool |
| `mimlys.lock`, `mimlys.holder` | Who has the store open | Nothing (a few bytes) |

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
- **Blobs, temporary files and interpreters** are small beside the others.

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
  let it go.
- It keeps the audit (the tasks' events) unless you add `--audit`.
- If one deletion fails, prune stops there, says which task failed and why, and
  names the tasks it did not touch.
- `mimlys task delete <task>` removes one finished task, after showing what
  will go and asking.
