-- Referrers table for OCI Distribution Spec v1.1 referrers API.
-- Populated when a manifest with a "subject" field is pushed.
CREATE TABLE oci_referrers (
    subject_digest  VARCHAR(71)  NOT NULL,
    repository      VARCHAR(500) NOT NULL,
    referrer_digest VARCHAR(71)  NOT NULL REFERENCES oci_manifests(digest) ON DELETE CASCADE,
    artifact_type   VARCHAR(255),
    annotations     JSONB,
    size_bytes      BIGINT       NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    PRIMARY KEY (subject_digest, referrer_digest)
);

CREATE INDEX ON oci_referrers(repository, subject_digest);
