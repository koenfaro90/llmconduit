-- Preserve every Fleet engine scrape, independently of host history buckets.
-- Payload is gzip/base64; the authenticated API expands selected series on read.
CREATE TABLE engine_metrics (
    endpoint TEXT NOT NULL,
    deployment TEXT NOT NULL,
    ts_ms BIGINT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (endpoint, deployment, ts_ms)
);
