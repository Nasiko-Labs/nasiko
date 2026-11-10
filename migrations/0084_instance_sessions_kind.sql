-- Sandbox-minutes metering: the hours meter records Running sandboxes beside container runs.
--
-- `kind` separates the two units: container rows are summed into container-hours, sandbox rows
-- into sandbox-minutes. Existing rows are containers. `sandbox_key` is the sandbox's canonical
-- name (`SandboxKey::name`), NULL for containers. `runtime` (still free text, see 0001) now also
-- takes the sandbox provider kind: 'docker', 'pod', 'substrate' or 'fake'.
ALTER TABLE agent_instance_sessions
    ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'container'
        CHECK (kind IN ('container', 'sandbox')),
    ADD COLUMN IF NOT EXISTS sandbox_key TEXT;
