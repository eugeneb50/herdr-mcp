ALTER TABLE queue_items ADD COLUMN delivery TEXT;

CREATE TABLE queue_consumers (
    queue TEXT PRIMARY KEY NOT NULL,
    owner TEXT NOT NULL,
    token INTEGER NOT NULL,
    until INTEGER NOT NULL
);

CREATE INDEX queue_items_reply ON queue_items (
    json_extract(payload,'$.re'), json_extract(payload,'$.delivery')
);
