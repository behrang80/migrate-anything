-- Migrate Anything workspace state, schema version 1.
-- Every table carries workspace_id so a future multi-workspace / PostgreSQL backend keeps the
-- same boundaries.

CREATE TABLE workspaces (
    id          TEXT PRIMARY KEY,
    created_at  TEXT NOT NULL
);

CREATE TABLE connections (
    workspace_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL,
    environment  TEXT NOT NULL,
    config_json  TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    PRIMARY KEY (workspace_id, name)
);

CREATE TABLE systems (
    workspace_id    TEXT NOT NULL,
    connection      TEXT NOT NULL,
    identity_json   TEXT NOT NULL,
    manifest_json   TEXT NOT NULL,
    discovered_at   TEXT NOT NULL,
    PRIMARY KEY (workspace_id, connection)
);

CREATE TABLE credentials (
    workspace_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    ciphertext   TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    PRIMARY KEY (workspace_id, name)
);

CREATE TABLE connectors (
    workspace_id  TEXT NOT NULL,
    connector_id  TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    PRIMARY KEY (workspace_id, connector_id)
);

CREATE TABLE connector_versions (
    workspace_id   TEXT NOT NULL,
    connector_id   TEXT NOT NULL,
    version        TEXT NOT NULL,
    status         TEXT NOT NULL,
    spec_json      TEXT NOT NULL,
    spec_sha256    TEXT NOT NULL,
    manifest_json  TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    activated_by   TEXT,
    activated_at   TEXT,
    PRIMARY KEY (workspace_id, connector_id, version)
);

CREATE TABLE connector_tests (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id  TEXT NOT NULL,
    connector_id  TEXT NOT NULL,
    version       TEXT NOT NULL,
    connection    TEXT NOT NULL,
    results_json  TEXT NOT NULL,
    passed        INTEGER NOT NULL,
    tested_at     TEXT NOT NULL
);

CREATE TABLE schemas (
    workspace_id  TEXT NOT NULL,
    connection    TEXT NOT NULL,
    schema_json   TEXT NOT NULL,
    discovered_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, connection)
);

CREATE TABLE extensions (
    workspace_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    sha256       TEXT NOT NULL,
    module       BLOB NOT NULL,
    status       TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    PRIMARY KEY (workspace_id, name)
);

CREATE TABLE plans (
    workspace_id  TEXT NOT NULL,
    plan_id       TEXT NOT NULL,
    plan_hash     TEXT NOT NULL,
    plan_json     TEXT NOT NULL,
    summary_md    TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    PRIMARY KEY (workspace_id, plan_id)
);

CREATE TABLE dry_runs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id  TEXT NOT NULL,
    plan_id       TEXT NOT NULL,
    plan_hash     TEXT NOT NULL,
    report_json   TEXT NOT NULL,
    created_at    TEXT NOT NULL
);

CREATE TABLE approval_requests (
    workspace_id  TEXT NOT NULL,
    plan_id       TEXT NOT NULL,
    plan_hash     TEXT NOT NULL,
    requested_by  TEXT NOT NULL,
    note          TEXT,
    requested_at  TEXT NOT NULL,
    PRIMARY KEY (workspace_id, plan_id, plan_hash)
);

CREATE TABLE approvals (
    workspace_id  TEXT NOT NULL,
    approval_id   TEXT NOT NULL,
    plan_id       TEXT NOT NULL,
    plan_hash     TEXT NOT NULL,
    record_json   TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    revoked       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id, approval_id)
);

CREATE TABLE jobs (
    workspace_id     TEXT NOT NULL,
    job_id           TEXT NOT NULL,
    plan_id          TEXT NOT NULL,
    plan_hash        TEXT NOT NULL,
    approval_id      TEXT NOT NULL,
    status           TEXT NOT NULL,
    control          TEXT NOT NULL DEFAULT 'none',
    created_at       TEXT NOT NULL,
    started_at       TEXT,
    finished_at      TEXT,
    heartbeat_at     TEXT,
    lease_owner      TEXT,
    lease_expires_at INTEGER,
    worker_pid       INTEGER,
    attempts         INTEGER NOT NULL DEFAULT 0,
    error            TEXT,
    PRIMARY KEY (workspace_id, job_id)
);

CREATE TABLE job_stages (
    workspace_id   TEXT NOT NULL,
    job_id         TEXT NOT NULL,
    stage          TEXT NOT NULL,
    seq            INTEGER NOT NULL,
    state          TEXT NOT NULL,
    counters_json  TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    PRIMARY KEY (workspace_id, job_id, stage)
);

CREATE TABLE checkpoints (
    workspace_id   TEXT NOT NULL,
    job_id         TEXT NOT NULL,
    stage          TEXT NOT NULL,
    cursor_json    TEXT,
    sink_token_json TEXT,
    inflight_json  TEXT,
    batches        INTEGER NOT NULL DEFAULT 0,
    updated_at     TEXT NOT NULL,
    PRIMARY KEY (workspace_id, job_id, stage)
);

CREATE TABLE id_mappings (
    workspace_id   TEXT NOT NULL,
    job_id         TEXT NOT NULL,
    source_object  TEXT NOT NULL,
    source_id      TEXT NOT NULL,
    dest_id        TEXT NOT NULL,
    dest_key       TEXT,
    PRIMARY KEY (workspace_id, job_id, source_object, source_id)
) WITHOUT ROWID;

CREATE INDEX id_mappings_dest ON id_mappings (workspace_id, job_id, source_object, dest_id);
CREATE INDEX id_mappings_key ON id_mappings (workspace_id, job_id, source_object, dest_key);

CREATE TABLE record_outcomes (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id   TEXT NOT NULL,
    job_id         TEXT NOT NULL,
    source_object  TEXT NOT NULL,
    source_id      TEXT NOT NULL,
    outcome        TEXT NOT NULL,
    message        TEXT,
    created_at     TEXT NOT NULL
);
CREATE INDEX record_outcomes_job ON record_outcomes (workspace_id, job_id, outcome);

CREATE TABLE exceptions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id   TEXT NOT NULL,
    job_id         TEXT NOT NULL,
    source_object  TEXT NOT NULL,
    source_id      TEXT NOT NULL,
    kind           TEXT NOT NULL,
    field          TEXT,
    value_json     TEXT,
    reason         TEXT NOT NULL,
    created_at     TEXT NOT NULL
);
CREATE INDEX exceptions_job ON exceptions (workspace_id, job_id, kind);

CREATE TABLE validation_results (
    workspace_id   TEXT NOT NULL,
    job_id         TEXT NOT NULL,
    result_json    TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    PRIMARY KEY (workspace_id, job_id)
);

CREATE TABLE audit_events (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id   TEXT NOT NULL,
    at             TEXT NOT NULL,
    actor          TEXT NOT NULL,
    action         TEXT NOT NULL,
    subject        TEXT,
    detail_json    TEXT NOT NULL
);

-- scratch tables used by streaming verification
CREATE TABLE verify_keys (
    workspace_id TEXT NOT NULL,
    job_id       TEXT NOT NULL,
    object       TEXT NOT NULL,
    key          TEXT NOT NULL
);
CREATE INDEX verify_keys_idx ON verify_keys (workspace_id, job_id, object, key);

CREATE TABLE verify_refs (
    workspace_id TEXT NOT NULL,
    job_id       TEXT NOT NULL,
    object       TEXT NOT NULL,
    field        TEXT NOT NULL,
    ref_object   TEXT NOT NULL,
    value        TEXT NOT NULL
);
