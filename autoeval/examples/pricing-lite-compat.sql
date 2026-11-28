-- Prepared for the two working model configurations. NOT applied by AutoEval.
-- Separate from pricing-compat.sql, which targets Hydrogen / 3.8 Flash.
-- Operator approval is required before executing this file against the server.
-- Verified 2026-11-28, Standard text/thinking-token prices:
-- https://aistudio.google.com/docs/pricing#gemini-3.1-flash-lite
-- https://aistudio.google.com/docs/pricing#gemini-3.5-flash-lite
-- No existing row is changed. Recheck after 2026-12-31. No usage is repriced.
BEGIN;
INSERT INTO model_pricing
  (provider, model, input_price_per_1m, output_price_per_1m,
   cache_read_price_per_1m, effective_until, notes)
SELECT 'gemini', 'gemini-3.1-flash-lite', 0.25, 1.50, 0.025,
       '2027-01-01T00:00:00Z'::timestamptz,
       'autoeval-lite-compat-20261128: Google Standard text prices; https://aistudio.google.com/docs/pricing#gemini-3.1-flash-lite'
WHERE NOT EXISTS (
  SELECT 1 FROM model_pricing WHERE provider='gemini' AND model='gemini-3.1-flash-lite'
  AND (effective_until IS NULL OR effective_until > now())
);
INSERT INTO model_pricing
  (provider, model, input_price_per_1m, output_price_per_1m,
   cache_read_price_per_1m, effective_until, notes)
SELECT 'gemini', 'gemini-3.5-flash-lite', 0.30, 2.50, 0.03,
       '2027-01-01T00:00:00Z'::timestamptz,
       'autoeval-lite-compat-20261128: Google Standard text prices; https://aistudio.google.com/docs/pricing#gemini-3.5-flash-lite'
WHERE NOT EXISTS (
  SELECT 1 FROM model_pricing WHERE provider='gemini' AND model='gemini-3.5-flash-lite'
  AND (effective_until IS NULL OR effective_until > now())
);
COMMIT;

-- Reversal affects only these additions; recorded usage is untouched:
-- DELETE FROM model_pricing WHERE notes LIKE 'autoeval-lite-compat-20261128:%';
