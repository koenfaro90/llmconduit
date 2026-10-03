-- Evidence stays bound to the tested recipe revision and host, independently of
-- the bounded live benchmark catalog. Later edits cannot inherit a pass.
CREATE TABLE recipe_evidence (
    id TEXT PRIMARY KEY,
    recipe_id TEXT NOT NULL,
    revision_digest TEXT NOT NULL,
    endpoint_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    ts_ms BIGINT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX idx_recipe_evidence_revision ON recipe_evidence(recipe_id, revision_digest, ts_ms);
