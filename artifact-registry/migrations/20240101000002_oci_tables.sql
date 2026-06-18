CREATE TABLE oci_manifests (
    digest      VARCHAR(71)  PRIMARY KEY,
    repository  VARCHAR(500) NOT NULL,
    reference   VARCHAR(255),
    media_type  VARCHAR(255) NOT NULL,
    content     JSONB        NOT NULL,
    size_bytes  BIGINT       NOT NULL,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX ON oci_manifests(repository);
CREATE INDEX ON oci_manifests(repository, reference);

CREATE TABLE oci_uploads (
    uuid         UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    repository   VARCHAR(500) NOT NULL,
    offset_bytes BIGINT       NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);
