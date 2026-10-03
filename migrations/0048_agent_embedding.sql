-- Persist agent embeddings so Stage 1 semantic routing doesn't re-embed the
-- catalog on every route() call or lose the cache on server restart.
ALTER TABLE agents
    ADD COLUMN embedding double precision[],
    ADD COLUMN embedding_content_hash bigint,
    ADD COLUMN embedded_at timestamptz;
