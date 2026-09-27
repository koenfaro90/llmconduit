-- Stable newest-first cursor pagination over the entire retained request history.
CREATE INDEX IF NOT EXISTS idx_requests_created_id_desc
    ON requests(created_at_ms DESC, id DESC);
