-- Consent-gated coding-agent onboarding: the server is the meeting point between the web UI, which
-- asks the user, and a local client (desktop app or `nasiko` CLI), which does the work on the
-- user's computer and reports how far it got.

-- One answer per user. No row means the user has not been asked yet ("unasked" on the wire), which
-- is why `decision` admits only the two answers a user can give.
CREATE TABLE coding_agent_consent (
    user_id    UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    decision   TEXT NOT NULL CHECK (decision IN ('granted', 'declined')),
    decided_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row per (user, device): a device's latest run overwrites its previous one. `agents` is the
-- per-agent progress the client reports, validated by the server before it is stored.
CREATE TABLE coding_agent_sync_runs (
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id   TEXT NOT NULL,
    device_name TEXT NOT NULL,
    client      TEXT NOT NULL CHECK (client IN ('desktop', 'cli')),
    phase       TEXT NOT NULL CHECK (phase IN (
        'awaiting_consent', 'discovering', 'connecting', 'importing', 'done', 'failed'
    )),
    agents      JSONB NOT NULL DEFAULT '[]'::jsonb,
    error       TEXT,
    started_at  TIMESTAMPTZ,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, device_id)
);

-- The only read: one user's runs updated within a recent window, newest first.
CREATE INDEX idx_coding_agent_sync_runs_user_updated
    ON coding_agent_sync_runs (user_id, updated_at DESC);
