-- no-transaction
-- flows.session_id NOT NULL, step 2 of 2 (step 1 is the NOT VALID check in 0079).
-- Runs outside a transaction so each statement takes and releases its own lock.
-- VALIDATE scans the table under SHARE UPDATE EXCLUSIVE, so reads and writes on
-- `flows` continue.
ALTER TABLE flows VALIDATE CONSTRAINT flows_session_id_not_null;
-- The validated check lets SET NOT NULL skip its table scan (PG 12+), so the
-- ACCESS EXCLUSIVE lock is catalog-only. 0058's partial idx_flows_session already
-- serves lookups by session_id; it is not rebuilt.
SET lock_timeout = '5s';
ALTER TABLE flows ALTER COLUMN session_id SET NOT NULL;
ALTER TABLE flows DROP CONSTRAINT flows_session_id_not_null;
RESET lock_timeout;
