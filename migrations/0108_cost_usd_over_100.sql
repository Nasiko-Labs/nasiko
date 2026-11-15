-- A turn's cost no longer has to be under $100. `chat_messages.cost_usd` and `flows.total_cost_usd` were
-- DECIMAL(10, 8), whose largest value is 99.99999999: one long coding-agent turn (126 model calls reading
-- 89M cached tokens on an Opus model, $148.53) failed with `numeric field overflow`, and since the whole
-- event batch fails with it, every turn queued behind it waited too. NUMERIC(18, 8) is what the budget
-- tables use. Raising the precision at the same scale needs no table rewrite, only a brief lock.
ALTER TABLE chat_messages ALTER COLUMN cost_usd TYPE NUMERIC(18, 8);
ALTER TABLE flows ALTER COLUMN total_cost_usd TYPE NUMERIC(18, 8);
