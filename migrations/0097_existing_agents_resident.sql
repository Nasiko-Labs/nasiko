-- Framework agents that exist when this runs stay resident.
--
-- An agent whose parking policy is `auto` is now parked once it is idle: the reaper scales its own
-- workload to zero, and the next call wakes it. Every agent created since migration 0081 defaulted
-- to `auto` without that meaning anything for a running agent, so its owner never chose to have
-- it parked. Those agents become `resident` and keep running as before; an owner who wants one
-- parked sets it back to `auto`.
--
-- A `parked` agent keeps `auto`: it was already parked, so parking is what its owner has.
-- Deleted agents are left alone, and agents created after this runs get the column default,
-- `auto`.
UPDATE agents
SET parking_policy = 'resident'
WHERE workload_kind = 'agent'
  AND parking_policy = 'auto'
  AND status <> 'parked'
  AND deleted_at IS NULL;
