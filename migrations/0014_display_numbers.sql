-- Operator-facing, stable request/session numbers. UUIDs remain the internal keys.
-- Existing history is numbered oldest first; future numbers never reuse a value
-- after history is cleared because display_counters is retained.
ALTER TABLE requests ADD COLUMN display_number BIGINT;
ALTER TABLE sessions ADD COLUMN display_number BIGINT;

WITH ranked AS (
    SELECT id, ROW_NUMBER() OVER (ORDER BY created_at_ms, id) AS number FROM requests
)
UPDATE requests SET display_number = (SELECT number FROM ranked WHERE ranked.id = requests.id);

WITH ranked AS (
    SELECT id, ROW_NUMBER() OVER (ORDER BY first_seen_ms, id) AS number FROM sessions
)
UPDATE sessions SET display_number = (SELECT number FROM ranked WHERE ranked.id = sessions.id);

CREATE UNIQUE INDEX idx_requests_display_number ON requests(display_number);
CREATE UNIQUE INDEX idx_sessions_display_number ON sessions(display_number);

CREATE TABLE display_counters (
    kind TEXT PRIMARY KEY,
    last_value BIGINT NOT NULL
);
INSERT INTO display_counters(kind, last_value)
    VALUES ('request', (SELECT COALESCE(MAX(display_number), 0) FROM requests));
INSERT INTO display_counters(kind, last_value)
    VALUES ('session', (SELECT COALESCE(MAX(display_number), 0) FROM sessions));
