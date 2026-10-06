-- 0056_workspace_catalog.sql (the workspace catalog)

CREATE TABLE agent_workspace_settings (
    agent_id               UUID PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    enabled                BOOLEAN NOT NULL DEFAULT true,
    quota_bytes            BIGINT  NOT NULL DEFAULT 5368709120,   -- 5 GiB per (agent,user)
    file_max_bytes         BIGINT  NOT NULL DEFAULT 1073741824,   -- 1 GiB via transfer URL
    inline_max_bytes       BIGINT  NOT NULL DEFAULT 6291456,      -- 6 MiB via save_file / inline read
    history_retention_days INT     NOT NULL DEFAULT 30,
    auto_keep_uploads      BOOLEAN NOT NULL DEFAULT false,
    result_links           BOOLEAN NOT NULL DEFAULT false,        -- add resource_link blocks (Claude Code)
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE workspace_files (                      -- the CURRENT pointer per path
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id        UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    rel_path        TEXT NOT NULL,
    blob_key        TEXT NOT NULL,                  -- workspace/{agent}/{user}/blobs/{upload_id}, immutable
    size_bytes      BIGINT NOT NULL,
    mime_type       TEXT NOT NULL,
    digest          TEXT,                           -- sha256 when known (relay: always)
    origin          TEXT NOT NULL CHECK (origin IN ('agent','upload','cascade','shared_copy')),
    origin_agent_id UUID,
    source_file_id  UUID,                           -- for shared_copy
    trace_id        TEXT NOT NULL,
    session_id      TEXT,
    state           TEXT NOT NULL DEFAULT 'kept' CHECK (state IN ('staged','kept')),
    staged_until    TIMESTAMPTZ,                    -- inbox TTL while state='staged'
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ,                    -- tombstone; cleared on re-write
    UNIQUE (agent_id, user_id, rel_path)
);
CREATE INDEX idx_workspace_files_trace   ON workspace_files(trace_id);
CREATE INDEX idx_workspace_files_owner   ON workspace_files(agent_id, user_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_workspace_files_staged  ON workspace_files(staged_until) WHERE state = 'staged';

CREATE TABLE workspace_file_versions (              -- history: superseded pointers
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id       UUID NOT NULL REFERENCES workspace_files(id) ON DELETE CASCADE,
    blob_key      TEXT NOT NULL,
    size_bytes    BIGINT NOT NULL,
    digest        TEXT,
    origin        TEXT NOT NULL,
    trace_id      TEXT NOT NULL,
    session_id    TEXT,
    created_at    TIMESTAMPTZ NOT NULL,
    superseded_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_workspace_file_versions_file ON workspace_file_versions(file_id, superseded_at DESC);

CREATE TABLE workspace_uploads (                    -- begin/finish_upload staging
    upload_id       UUID PRIMARY KEY,
    agent_id        UUID NOT NULL,
    user_id         UUID NOT NULL,
    rel_path        TEXT NOT NULL,
    declared_size   BIGINT,
    mime_type       TEXT,
    blob_key        TEXT NOT NULL,
    mode            TEXT NOT NULL CHECK (mode IN ('direct','relay')),
    capability_hash TEXT,
    state           TEXT NOT NULL CHECK (state IN ('pending','received','finished','abandoned')),
    received_size   BIGINT,
    received_digest TEXT,
    trace_id        TEXT NOT NULL,
    session_id      TEXT,
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at     TIMESTAMPTZ
);
CREATE INDEX idx_workspace_uploads_expiry ON workspace_uploads(expires_at) WHERE state IN ('pending','received');

CREATE TABLE workspace_file_grants (                -- view-only sharing (A6)
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id           UUID NOT NULL REFERENCES workspace_files(id) ON DELETE CASCADE,
    grant_type        TEXT NOT NULL CHECK (grant_type IN ('user','team','department','public')),
    grantee_id        TEXT NOT NULL,
    granted_by        UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    granting_agent_id UUID,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (file_id, grant_type, grantee_id),
    CONSTRAINT chk_workspace_grant_public_sentinel CHECK (
        (grant_type = 'public' AND grantee_id = '*') OR (grant_type <> 'public' AND grantee_id <> '*'))
);
CREATE INDEX idx_workspace_file_grants_grantee ON workspace_file_grants(grant_type, grantee_id);

ALTER TABLE chat_message_files
    ADD COLUMN IF NOT EXISTS workspace_file_id UUID REFERENCES workspace_files(id) ON DELETE SET NULL;
