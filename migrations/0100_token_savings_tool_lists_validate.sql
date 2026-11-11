-- Validate the widened `layer` check added NOT VALID by 0099. Its own migration because
-- VALIDATE CONSTRAINT takes only SHARE UPDATE EXCLUSIVE: reads and writes continue while it scans,
-- which they would not inside 0099's transaction, still holding that migration's exclusive lock.
--
-- cold: compatible (validation scans hot rows only; cold rows are untouched)
ALTER TABLE token_savings
    VALIDATE CONSTRAINT token_savings_layer_check;
