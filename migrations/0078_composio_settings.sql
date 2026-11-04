-- Composio API key, set from the admin settings page instead of the COMPOSIO_API_KEY env.
-- Encrypted like the other platform-wide secrets on this row (the embedding key,
-- the OIDC client secret). NULL means "use the env value, if any".
ALTER TABLE settings ADD COLUMN IF NOT EXISTS composio_api_key_encrypted TEXT;
