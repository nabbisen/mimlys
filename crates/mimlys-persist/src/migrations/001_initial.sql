-- Schema version 1 (external design §16.1). Records are stored as JSON with
-- the columns needed by queries and consistency rules beside them.
CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version (version) VALUES (1);

CREATE TABLE tasks (
    task_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    goal TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    policy_hash TEXT NOT NULL,
    decision_schema_version INTEGER NOT NULL,
    runtime_version TEXT NOT NULL,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL
);

CREATE TABLE steps (
    step_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (task_id),
    idx INTEGER NOT NULL,
    outcome TEXT,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL,
    UNIQUE (task_id, idx)
);

CREATE TABLE actions (
    action_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (task_id),
    step_id TEXT NOT NULL REFERENCES steps (step_id),
    status TEXT NOT NULL,
    approval_required INTEGER NOT NULL,
    action_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL
);
CREATE INDEX actions_task ON actions (task_id, created_at);

CREATE TABLE executions (
    execution_id TEXT PRIMARY KEY,
    action_id TEXT NOT NULL REFERENCES actions (action_id),
    attempt INTEGER NOT NULL,
    termination TEXT NOT NULL,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL
);

CREATE TABLE observations (
    observation_id TEXT PRIMARY KEY,
    action_id TEXT NOT NULL REFERENCES actions (action_id),
    created_at TEXT NOT NULL,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL
);

CREATE TABLE approvals (
    approval_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (task_id),
    action_id TEXT NOT NULL REFERENCES actions (action_id),
    status TEXT NOT NULL,
    action_hash TEXT NOT NULL,
    grant_set_hash TEXT NOT NULL,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL
);

CREATE TABLE questions (
    question_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (task_id),
    status TEXT NOT NULL,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL
);

CREATE TABLE grants (
    grant_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (task_id),
    status TEXT NOT NULL,
    record TEXT NOT NULL,
    record_version INTEGER NOT NULL
);

CREATE TABLE plan_versions (
    task_id TEXT NOT NULL REFERENCES tasks (task_id),
    version INTEGER NOT NULL,
    record TEXT NOT NULL,
    PRIMARY KEY (task_id, version)
);

CREATE TABLE blobs (
    blob_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    size INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE events (
    position INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    at TEXT NOT NULL,
    name TEXT NOT NULL,
    task_id TEXT,
    step_id TEXT,
    action_id TEXT,
    record TEXT NOT NULL
);
CREATE INDEX events_task ON events (task_id, position);
