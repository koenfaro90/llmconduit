-- Fleet observations survive page closes and controller restarts. No automatic
-- retention policy: operators explicitly own storage lifecycle.
CREATE TABLE host_metrics (
    backend TEXT NOT NULL,
    ts_ms BIGINT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (backend, ts_ms)
);
