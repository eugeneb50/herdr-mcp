CREATE TABLE queue_items (
    id TEXT PRIMARY KEY,
    queue TEXT NOT NULL,
    seq INTEGER NOT NULL,
    status TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    claims INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    dedupe_key TEXT,
    coalesce_key TEXT,
    payload TEXT NOT NULL,
    requester TEXT,
    waiters TEXT NOT NULL DEFAULT '[]',
    run_id TEXT,
    lease_until INTEGER,
    result TEXT,
    enqueued_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER
);
CREATE UNIQUE INDEX queue_items_seq ON queue_items (queue, seq);
CREATE INDEX queue_items_runnable ON queue_items (queue, seq) WHERE status = 'queued';
CREATE INDEX queue_items_leased ON queue_items (lease_until) WHERE status = 'running';
CREATE UNIQUE INDEX queue_items_dedupe ON queue_items (queue, dedupe_key)
    WHERE dedupe_key IS NOT NULL AND status IN ('queued', 'running');

CREATE TABLE queue_events (
    item_id TEXT NOT NULL REFERENCES queue_items (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    data TEXT,
    PRIMARY KEY (item_id, seq)
);
