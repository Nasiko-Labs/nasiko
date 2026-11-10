-- The key a session's sandbox runs under, recorded before the sandbox is acquired.
--
-- `sandbox_ref` is only written once the provider has answered, so for the whole of a first
-- acquire nothing on the row says which sandbox is being created. Anything that has to find a
-- session from its sandbox (the orphan pass, metering) matched on `sandbox_ref`, or on the
-- session id when it is a UUID, and so missed a first turn on a `ses_<hex>` id and could release
-- its sandbox mid-acquire. `sandbox_key` is `SandboxKey::name()`, written under the turn's fence
-- before `acquire`.
--
-- One transaction, so a plain CREATE INDEX rather than CONCURRENTLY (which cannot share a
-- migration with the ALTER, since sqlx runs a file's statements as one block). The index is
-- partial and the column starts NULL on every row, so there is nothing to scan or lock for.
ALTER TABLE chat_sessions ADD COLUMN IF NOT EXISTS sandbox_key TEXT;
CREATE INDEX IF NOT EXISTS idx_chat_sessions_sandbox_key
    ON chat_sessions (sandbox_key)
    WHERE sandbox_key IS NOT NULL;
