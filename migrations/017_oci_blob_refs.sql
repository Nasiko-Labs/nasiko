-- =============================================================================
-- OCI blob reference counting (P1-3): blobs are stored at a flat, globally
-- content-addressed key (see storage.rs's blob_key) with no linkage to the
-- repositories that reference them. Without this table, deleting a blob from
-- one repo can destroy a layer another repo still needs, and any repo owner
-- can GET/HEAD any blob in the registry by digest (confidentiality leak) —
-- `check_repo_access` only verifies repo-level ownership, never whether the
-- caller's repo ever actually referenced that specific digest.
-- =============================================================================

CREATE TABLE oci_blob_refs (
    digest      TEXT        NOT NULL,
    repository  TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (digest, repository)
);

-- Fan-out check on delete: "does any OTHER repo still reference this digest".
CREATE INDEX idx_blob_refs_digest ON oci_blob_refs(digest);

-- =============================================================================
-- Adjacent bug fixed in the same migration: oci_manifests.digest was a global
-- PRIMARY KEY, not scoped per repository. put_manifest's
-- `ON CONFLICT (digest) DO UPDATE SET reference = ...` meant a second repo
-- pushing byte-identical manifest content to a digest another repo already
-- owned would silently overwrite that OTHER repo's tag pointer while leaving
-- `repository` unchanged — repo B's push looked successful but was
-- unretrievable under B's own name, and A's tag was hijacked. Each repo now
-- gets its own row for the same digest, matching how blob storage dedup
-- already works one layer down.
-- =============================================================================

ALTER TABLE oci_manifests DROP CONSTRAINT oci_manifests_pkey;
ALTER TABLE oci_manifests ADD PRIMARY KEY (repository, digest);

-- =============================================================================
-- Backfill: every blob pushed before this migration has zero oci_blob_refs
-- rows. Without this, the migration is a hard outage — every existing image
-- becomes immediately un-fetchable (404 on pull) and un-deletable (fail-closed
-- 404) the moment it ships. Extract every referenced config/layer digest from
-- each existing manifest's stored content (manifest lists/indexes have no
-- "config"/"layers" keys at this level, so they naturally contribute zero
-- rows here — their child manifests, pushed separately, backfill correctly).
-- =============================================================================

INSERT INTO oci_blob_refs (digest, repository)
SELECT DISTINCT content::jsonb -> 'config' ->> 'digest', repository
FROM oci_manifests
WHERE content::jsonb -> 'config' ->> 'digest' IS NOT NULL
ON CONFLICT (digest, repository) DO NOTHING;

INSERT INTO oci_blob_refs (digest, repository)
SELECT DISTINCT layer_digest, repository
FROM (
    SELECT repository, jsonb_array_elements(content::jsonb -> 'layers') ->> 'digest' AS layer_digest
    FROM oci_manifests
    WHERE jsonb_typeof(content::jsonb -> 'layers') = 'array'
) backfill
WHERE layer_digest IS NOT NULL
ON CONFLICT (digest, repository) DO NOTHING;
