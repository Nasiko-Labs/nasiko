-- =============================================================================
-- OCI registry storage: manifests, tags, uploads, referrers, blob reference
-- counting, blob GC, and per-workload pull credentials.
--
-- The central modelling rule: **content and pointers are separate tables.**
-- A manifest is immutable content named by the hash of its bytes; a tag is a
-- mutable pointer at one. Folding both into a single row loses data whichever
-- column you key on:
--
--   * keyed on `digest` alone      — pushing the same content under a second
--                                    tag overwrites the first tag's row (tag
--                                    hijack, cross-repository data loss);
--   * keyed on `(repository, tag)` — repointing a tag drops the only row for
--                                    the old digest, so pulling that digest
--                                    404s and any index pinned to it breaks.
--
-- `oci_manifests` (content, keyed by digest) + `oci_tags` (pointers, keyed by
-- tag) has neither failure: many tags may point at one retained manifest, and
-- repointing a tag never destroys content.
-- =============================================================================

-- Immutable content store. One row per (repository, digest); `digest` is the
-- hash of `content`, so a row's bytes never change once written.
--
-- Manifests are scoped per repository: the same digest may be pushed by many
-- repos (content-addressed dedup happens one layer down, at blob storage), and
-- a global digest PK would let one repo's push hijack another's tag pointer.
CREATE TABLE oci_manifests (
    digest TEXT NOT NULL,
    repository TEXT NOT NULL,
    media_type TEXT NOT NULL,
    -- stored as TEXT (not JSONB) so the served bytes hash back to `digest`
    content TEXT NOT NULL,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (repository, digest)
);

-- Mutable tag pointers. `tags/list` reads this; repointing a tag is an UPDATE
-- of `digest` here and leaves the old manifest row intact. CASCADE so deleting
-- a manifest takes every tag that pointed at it.
CREATE TABLE oci_tags (
    repository TEXT        NOT NULL,
    -- OCI tag grammar: [a-zA-Z0-9_][a-zA-Z0-9._-]{0,127} — never contains ':'
    tag        TEXT        NOT NULL,
    digest     TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (repository, tag),
    FOREIGN KEY (repository, digest)
        REFERENCES oci_manifests(repository, digest) ON DELETE CASCADE
);

-- "which tags point at this manifest" — the delete-by-digest path.
CREATE INDEX idx_oci_tags_repo_digest ON oci_tags(repository, digest);

-- In-progress chunked uploads. Buffers live in server memory; this table only
-- tracks the session (repository + resumable offset).
CREATE TABLE oci_uploads (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    repository TEXT NOT NULL,
    offset_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- OCI 1.1 referrers (SBOMs, signatures, attestations attached to a manifest).
-- Keyed by repository as well as the digest pair: the same referrer for the
-- same subject may legitimately be published in two repositories, and a
-- repository-blind key would silently drop the second one (ON CONFLICT DO
-- NOTHING against the first repository's row), leaving that repository's
-- referrers endpoint empty. The FK on the referring manifest reaps referrers
-- when their manifest is deleted.
CREATE TABLE oci_referrers (
    subject_digest TEXT NOT NULL,
    repository TEXT NOT NULL,
    referrer_digest TEXT NOT NULL,
    artifact_type TEXT,
    annotations JSONB,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (repository, subject_digest, referrer_digest),
    FOREIGN KEY (repository, referrer_digest)
        REFERENCES oci_manifests(repository, digest) ON DELETE CASCADE
);
CREATE INDEX idx_referrers_subject ON oci_referrers(repository, subject_digest);

-- Blob reference counting: blobs live at a flat, globally content-addressed
-- key with no linkage to the repositories that reference them. This table
-- provides that linkage so (a) deleting a blob from one repo cannot destroy a
-- layer another repo still needs, and (b) a repo owner can only GET/HEAD
-- blobs their repo actually references.
--
-- Written both when a blob upload completes and when a manifest referencing it
-- is pushed, so a blob is claimed from the moment it exists — an upload that is
-- never referenced by any manifest stays deletable by its uploader instead of
-- becoming permanently undeletable dead storage.
CREATE TABLE oci_blob_refs (
    digest TEXT NOT NULL,
    repository TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (digest, repository)
);
-- Fan-out check on delete: "does any OTHER repo still reference this digest".
CREATE INDEX idx_blob_refs_digest ON oci_blob_refs(digest);

-- Pending physical blob deletions. A Postgres transaction cannot make an S3
-- delete atomic, so the two are decoupled: dropping the last reference commits
-- a tombstone here and nothing else, then a sweep re-checks the reference
-- count under a digest-scoped advisory lock and removes the bytes. The commit
-- is the only durable decision, so failures are one-sided — a crash or a
-- failed storage call leaves reclaimable bytes with a tombstone still queued,
-- never a committed reference pointing at bytes that are already gone.
CREATE TABLE oci_blob_gc (
    digest       TEXT        PRIMARY KEY,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Per-workload credentials for pulling an image from the built-in OCI
-- registry out of a real Kubernetes cluster: kubelet/containerd need the
-- `kubernetes.io/dockerconfigjson` (HTTP Basic) shape, which the registry's
-- normal bearer-JWT auth doesn't fit. One row per workload — minted on first
-- deploy, reused across update/rollback/restart, revoked on destroy. Scoped
-- per-workload so a leaked credential only exposes one workload's image.
--
-- The column is named agent_id for backward compatibility but is effectively
-- a workload_id: it also holds MCP connector UUIDs (uploaded MCP servers need
-- pull credentials the same way agents do), which is why it has no FK.
CREATE TABLE oci_pull_credentials (
    agent_id UUID PRIMARY KEY,
    username TEXT NOT NULL,
    token_hash VARCHAR(64) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX idx_oci_pull_credentials_token_hash_active
    ON oci_pull_credentials(token_hash)
    WHERE revoked_at IS NULL;
