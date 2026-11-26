WITH mail AS (
    SELECT id, substr(queue, 6) AS label, status, payload, result, max_attempts
    FROM queue_items WHERE queue GLOB 'mail-?*'
), gauges AS (
    SELECT label, sum(status = 'queued') AS depth, sum(status = 'running') AS in_flight
    FROM mail GROUP BY label
), activity AS (
    SELECT m.label, max(e.at) AS last_activity_at,
        count(*) FILTER (
            WHERE (:since IS NULL OR e.at >= :since) AND e.kind = 'delivered'
              AND json_type(e.data, '$.submitted') = 'true'
        ) AS delivered,
        count(*) FILTER (
            WHERE (:since IS NULL OR e.at >= :since) AND e.kind = 'failed'
        ) AS failed,
        count(*) FILTER (
            WHERE (:since IS NULL OR e.at >= :since)
              AND (e.kind = 'dead' OR (
                  e.kind = 'stalled' AND json_extract(e.data, '$.attempt') >= m.max_attempts
              ))
        ) AS dead
    FROM mail m JOIN queue_events e ON e.item_id = m.id GROUP BY m.label
), replies AS (
    SELECT m.label, d.item_id, d.seq,
        CASE WHEN r.enqueued_at >= d.at THEN r.enqueued_at - d.at END AS ms
    FROM mail m
    JOIN queue_events c ON c.item_id = m.id AND c.kind = 'done'
    JOIN queue_events d ON d.item_id = c.item_id AND d.seq = (
        SELECT max(p.seq) FROM queue_events p
        WHERE p.item_id = c.item_id AND p.seq < c.seq AND p.kind = 'delivered'
          AND json_type(p.data, '$.submitted') = 'true'
          AND json_extract(p.data, '$.claim') = json_extract(c.data, '$.claim')
          AND json_extract(p.data, '$.delivery') = json_extract(m.result, '$.delivery')
    )
    LEFT JOIN queue_items r ON r.id = json_extract(m.result, '$.reply')
    WHERE m.status = 'done' AND json_extract(m.payload, '$.status') = 'task'
      AND json_type(m.result, '$.reply') = 'text' AND (:since IS NULL OR c.at >= :since)
), reply_counts AS (
    SELECT label, count(*) AS replied FROM replies GROUP BY label
), ranked AS (
    SELECT label, ms, row_number() OVER (PARTITION BY label ORDER BY ms, item_id, seq) AS rn,
        count(*) OVER (PARTITION BY label) AS n
    FROM replies WHERE ms IS NOT NULL
), latency AS (
    SELECT label,
        avg(ms) FILTER (WHERE rn IN ((n + 1) / 2, (n + 2) / 2)) AS median_reply_ms,
        max(ms) FILTER (WHERE rn = (9 * n + 9) / 10) AS p90_reply_ms
    FROM ranked GROUP BY label
)
SELECT g.label AS "to", g.depth, g.in_flight,
    coalesce(a.delivered, 0) AS delivered, coalesce(t.replied, 0) AS replied,
    coalesce(a.failed, 0) AS failed, coalesce(a.dead, 0) AS dead,
    l.median_reply_ms, l.p90_reply_ms, a.last_activity_at
FROM gauges g
LEFT JOIN activity a USING (label)
LEFT JOIN reply_counts t USING (label)
LEFT JOIN latency l USING (label)
ORDER BY g.label;
