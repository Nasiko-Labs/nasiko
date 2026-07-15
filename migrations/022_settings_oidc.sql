-- Admin-configurable OIDC SSO settings, stored alongside the existing
-- `settings` singleton row rather than a new table. Non-secret fields are
-- plain columns (same pattern as router_model/registry_url etc.);
-- `oidc_client_secret_encrypted` holds an AES-256-GCM ciphertext (see
-- `SecretsCrypto::for_platform_settings`) — the plaintext secret is never
-- persisted and never leaves the encrypted column.
ALTER TABLE settings
    ADD COLUMN oidc_issuer_url TEXT,
    ADD COLUMN oidc_client_id TEXT,
    ADD COLUMN oidc_client_secret_encrypted TEXT,
    ADD COLUMN oidc_redirect_uri TEXT,
    ADD COLUMN oidc_scopes TEXT,
    ADD COLUMN oidc_provider_label TEXT;
