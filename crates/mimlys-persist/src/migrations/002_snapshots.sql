-- Schema version 2 (RFC 006): snapshots, object reference counts, workspace
-- lifecycle and promotions. Objects and manifests are files under the data
-- directory; rows reference only durable files (REQ-PERS-009).

CREATE TABLE snapshots (
    snapshot_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (task_id),
    reason TEXT NOT NULL,
    manifest_hash TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    files INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    action_id TEXT,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL
);
CREATE INDEX snapshots_task ON snapshots (task_id, created_at);

CREATE TABLE objects (
    hash TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    refs INTEGER NOT NULL
);

CREATE TABLE workspaces (
    task_id TEXT PRIMARY KEY REFERENCES tasks (task_id),
    state TEXT NOT NULL,
    changed_at TEXT NOT NULL
);

CREATE TABLE promotions (
    promotion_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (task_id),
    backup_snapshot_id TEXT NOT NULL REFERENCES snapshots (snapshot_id),
    applied_at TEXT NOT NULL,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL
);
CREATE INDEX promotions_task ON promotions (task_id, applied_at);
