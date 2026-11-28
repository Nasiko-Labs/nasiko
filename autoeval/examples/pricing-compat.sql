-- Optional operator action for the old prebuilt server. NOT run by AutoEval.
-- It cannot match the context-suffixed Portkey prices for these native model IDs.
-- Verified 2026-11-28 against Google's Standard text/thinking-token prices:
-- https://aistudio.google.com/docs/pricing#gemini-4-hydrogen
-- https://aistudio.google.com/docs/pricing#gemini-3.8-flash
-- No existing pricing row is changed. Flash promotional rates expire 2027-01-01.
BEGIN;
INSERT INTO model_pricing
  (provider, model, input_price_per_1m, output_price_per_1m,
   cache_read_price_per_1m, effective_until, notes)
SELECT 'gemini', 'gemini-4-hydrogen', 1.50, 7.50, 0.075,
       '2027-01-01T00:00:00Z'::timestamptz,
       'autoeval-compat-20261128: Google Standard pricing; https://aistudio.google.com/docs/pricing#gemini-4-hydrogen'
WHERE NOT EXISTS (
  SELECT 1 FROM model_pricing WHERE provider='gemini' AND model='gemini-4-hydrogen'
  AND (effective_until IS NULL OR effective_until > now())
);
INSERT INTO model_pricing
  (provider, model, input_price_per_1m, output_price_per_1m,
   cache_read_price_per_1m, effective_until, notes)
SELECT 'gemini', 'gemini-3.8-flash', 0.75, 3.75, 0.075,
       '2027-01-01T00:00:00Z'::timestamptz,
       'autoeval-compat-20261128: Google Standard pricing through 2026-12-31; https://aistudio.google.com/docs/pricing#gemini-3.8-flash'
WHERE NOT EXISTS (
  SELECT 1 FROM model_pricing WHERE provider='gemini' AND model='gemini-3.8-flash'
  AND (effective_until IS NULL OR effective_until > now())
);
COMMIT;

-- To reverse only these additions later (existing usage amounts stay as recorded):
-- DELETE FROM model_pricing WHERE notes LIKE 'autoeval-compat-20261128:%';
