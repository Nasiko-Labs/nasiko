-- The credential a harness sandbox carries.
--
-- A sandbox's environment is rebuilt from the platform's spec on every acquire (a resume after a
-- provider restart, or a re-create, applies it again), so the token in it must be one the
-- platform can hand back out: it is kept encrypted at rest (`sandbox_token_enc`, AES-GCM with
-- the platform master key, as agent secrets are), with its `jti` beside it so it can be revoked
-- without decrypting. Both are NULL until the first turn mints the token, and again once it is
-- revoked.
ALTER TABLE chat_sessions ADD COLUMN IF NOT EXISTS sandbox_token_jti TEXT;
ALTER TABLE chat_sessions ADD COLUMN IF NOT EXISTS sandbox_token_enc BYTEA;
