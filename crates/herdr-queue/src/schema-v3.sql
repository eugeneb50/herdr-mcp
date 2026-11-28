-- Reserved keys (`Queue::push_unique`, `Queue::find_key`) are looked up across every queue and status.
CREATE INDEX queue_items_key ON queue_items (dedupe_key) WHERE dedupe_key IS NOT NULL;
