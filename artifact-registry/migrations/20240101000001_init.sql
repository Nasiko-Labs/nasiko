CREATE TABLE artifacts (
    id            UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    owner         VARCHAR(255) NOT NULL,
    name          VARCHAR(255) NOT NULL,
    version       VARCHAR(50)  NOT NULL,
    artifact_type VARCHAR(50)  NOT NULL,
    status        VARCHAR(20)  NOT NULL DEFAULT 'preview',
    description   TEXT,
    metadata      JSONB        NOT NULL DEFAULT '{}',
    oci_digest    VARCHAR(71),
    size_bytes    BIGINT,
    tags          TEXT[]       NOT NULL DEFAULT '{}',
    framework     VARCHAR(50),
    license       VARCHAR(50),
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    UNIQUE(owner, name, version)
);

CREATE INDEX ON artifacts(artifact_type, status);
CREATE INDEX ON artifacts(owner, name);
CREATE INDEX ON artifacts USING GIN(tags);
