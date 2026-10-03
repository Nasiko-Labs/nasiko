-- Effect factors for the layers whose counterfactual is unobservable.
--
-- Payload compression, tool-result compression, history compression and context selection
-- are subtractions: the code holds both the original and the reduced form, so the saving is
-- measured. The brevity directive and the minimal-code ladder are not. We appended text and
-- the model wrote something; what it would have written otherwise exists nowhere.
--
-- For those, one percentage is applied to a *counted* volume of eligible traffic. The
-- assumption is the rate, never the denominator — which is why a seeded category still
-- responds correctly to the feature being enabled or disabled.
--
-- Factors are applied at READ time. Changing one rewrites every historical figure on the
-- dashboard, so `basis` and `measured_at` are served alongside every derived number.
CREATE TABLE optimization_effect_factors (
    layer            TEXT PRIMARY KEY
                     CHECK (layer IN ('brevity', 'minimal_code', 'prompt_comments')),
    -- Negative = reduction. Separate input/output because the layers act on different sides:
    -- brevity shortens completions, the minimal-code ladder changes the whole turn.
    output_token_delta_pct DOUBLE PRECISION NOT NULL,
    input_token_delta_pct  DOUBLE PRECISION NOT NULL DEFAULT 0,
    -- 'seed_default' until a holdout run overwrites the row. Served verbatim as the API's
    -- `basis`, so a consumer never has to infer how a number was arrived at.
    basis            TEXT NOT NULL CHECK (basis IN ('seed_default', 'fixture')),
    confidence_pct   DOUBLE PRECISION,  -- ± half-width; NULL for a seed
    sample_count     INTEGER,           -- holdout samples behind the figure; NULL for a seed
    model            TEXT,              -- the model the factor was measured against; NULL for a seed
    measured_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Where the number came from, in one sentence. Served verbatim, so the basis of an
    -- assumed figure travels with it to every consumer rather than living only here.
    notes            TEXT NOT NULL
);

-- Seeds. Optimistic by product decision, 2026-10-03, and deliberately short-lived: the
-- holdout replaces each with a measured factor once it clears the sample floor.
--
-- This INSERT must NOT be edited to change a value later. Revising a factor is an UPDATE
-- against the live row, so that the running value's `basis` and `measured_at` stay truthful
-- instead of reverting to the seed on a fresh deploy.
INSERT INTO optimization_effect_factors
    (layer, output_token_delta_pct, input_token_delta_pct, basis, notes) VALUES
  ('brevity', -35.0, 0.0, 'seed_default',
   'Seed, roughly half of Caveman''s published ~65% output saving. Above our PRD §6 target of -8%, which was written as a success threshold rather than a prediction. Replaced by the holdout once the sample floor is cleared.'),
  ('minimal_code', -30.0, -30.0, 'seed_default',
   'Seed, above Ponytail''s own published -22% token figure and below their ~54% code-volume figure (a different quantity). Two reasons to expect the true value is lower: their -22% came from audit/review skills while our Phase 1 is the ladder alone, and the addendum costs ~100-200 tokens on every turn. Replaced by the holdout.'),
  ('prompt_comments', 0.0, -8.0, 'seed_default',
   'Placeholder. Becomes a measured subtraction (bytes stripped) once the coding agent emits its span attribute.');
