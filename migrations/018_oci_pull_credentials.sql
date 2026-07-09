-- Per-agent credentials for pulling that agent's own image out of the
-- built-in OCI registry from a real Kubernetes cluster. The registry's
-- normal auth (bearer-JWT, the CP session token) doesn't fit the
-- `kubernetes.io/dockerconfigjson` shape kubelet/containerd need for
-- imagePullSecrets, so pulls authenticate via HTTP Basic auth against this
-- table instead. One row per agent (not per-deploy) — minted once on an
-- agent's first deploy, reused across update/rollback/restart, revoked on
-- agent destroy. Scoped per-agent (not shared cluster-wide) so a leaked
-- credential only exposes one agent's image.

CREATE TABLE oci_pull_credentials (
    agent_id    UUID PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    username    TEXT NOT NULL,
    token_hash  VARCHAR(64) NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at  TIMESTAMPTZ
);

CREATE UNIQUE INDEX idx_oci_pull_credentials_token_hash_active
    ON oci_pull_credentials(token_hash)
    WHERE revoked_at IS NULL;
