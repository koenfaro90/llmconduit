-- Retire the old inferred-session shapes.
--
-- The old linker attributed undeclared requests to a per-client "bucket"
-- session node and chained every unmatched conversation after the previous
-- head, merging thousands of unrelated conversations into single nodes. The
-- new linker content-matches undeclared requests into per-conversation
-- inferred nodes (see src/sessions.rs) and never infers sub-sessions.
--
-- 1. Requests previously attached to a root inferred bucket node: their
--    session attribution was fabricated (the chain parent was simply the
--    previous request from the same client key, unrelated or not), so detach
--    them entirely — NULL session_id and lineage columns.
-- 2. Requests attached to an inferred sub-session node: reattach to the
--    NEAREST declared ancestor (highest depth), keeping their lineage (the
--    chain parent was a real predecessor inside that conversation). When no
--    declared ancestor exists (a child of a bucket node) the request is
--    detached like the bucket case.
-- 3. Drop every inferred session row; the new linker creates conversation
--    nodes going forward and warm-up seeds only fresh ones.

UPDATE requests
SET session_id = NULL,
    chain_parent_request_id = NULL,
    divergence_kind = NULL,
    divergence_index = NULL,
    shared_prefix_items = NULL,
    cache_bust = NULL
WHERE session_id IN (
    SELECT id FROM sessions
    WHERE kind = 'inferred' AND parent_id IS NULL
);

UPDATE requests
SET session_id = (
    WITH RECURSIVE ancestors(id, depth) AS (
        SELECT s.parent_id, s.depth
        FROM sessions s
        WHERE s.id = requests.session_id AND s.parent_id IS NOT NULL
        UNION ALL
        SELECT s2.parent_id, s2.depth
        FROM sessions s2
        JOIN ancestors a ON s2.id = a.id AND s2.parent_id IS NOT NULL
    )
    SELECT anc.id
    FROM ancestors anc
    JOIN sessions s3 ON s3.id = anc.id
    WHERE s3.kind = 'declared'
    ORDER BY anc.depth DESC
    LIMIT 1
)
WHERE session_id IN (
    SELECT id FROM sessions
    WHERE kind = 'inferred' AND parent_id IS NOT NULL
);

DELETE FROM sessions WHERE kind = 'inferred';

-- Conversation-node warm-up lookup: the client's recent inferred roots
-- (recent_conversations filters by harness + client key over this shape).
CREATE INDEX IF NOT EXISTS idx_sessions_conversations
    ON sessions(harness, last_seen_ms) WHERE kind = 'inferred' AND parent_id IS NULL;
