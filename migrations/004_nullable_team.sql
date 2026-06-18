-- OSS mode is single-user, no teams. Cloud enforces team ownership at app level.
ALTER TABLE agents ALTER COLUMN owner_team_id DROP NOT NULL;
