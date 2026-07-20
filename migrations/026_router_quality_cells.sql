-- Learned quality cells for the smart model router's Thompson-sampling tier selection (S5).
--
-- The classifier buckets each query into a request type, then treats the three strength
-- tiers as bandit arms and Thompson-samples one from a Beta posterior. This table is that
-- posterior's memory: for a (provider, tier, request_type) it stores a running mean of the
-- observed reward and how many observations back it. Rewards come from the user's next-turn
-- reaction (approval → 1.0, complaint → 0.0), credited by route_model.
--
-- Cells are scoped PER PROVIDER (tiers/costs are provider-specific) and shared across
-- conversations, so learning from one conversation improves the next one's cold-start pick.
--
-- The router (PgCellStore) treats this as a latency/quality optimisation, never a
-- correctness dependency: a read failure degrades to cold-start priors (an empty cell set)
-- and a write failure is dropped, so an absent/unreachable table stalls learning but never
-- breaks routing. Starts empty — there are no seed rows; every cell is earned from feedback.
CREATE TABLE IF NOT EXISTS router_quality_cells (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider TEXT NOT NULL,
    tier SMALLINT NOT NULL CHECK (tier IN (1, 2, 3)),
    request_type TEXT NOT NULL,
    -- Running mean of observed reward in [0, 1]; see update_cell in oss/llm-router.
    quality_mean DOUBLE PRECISION NOT NULL,
    -- Effective sample count, capped by the router at MAX_SAMPLES (200).
    samples BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, tier, request_type)
);

-- The (provider) prefix of the unique index already serves PgCellStore's per-provider load.

CREATE TRIGGER trg_router_quality_cells_updated_at BEFORE UPDATE ON router_quality_cells
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
