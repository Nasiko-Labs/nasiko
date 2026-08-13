-- Separate OCI manifest CONTENT from the mutable TAG pointers that name it.
--
-- `oci_manifests` carried both: a content row plus a `reference` column holding
-- one tag. With `PRIMARY KEY (repository, digest)` the upsert on push was
-- `DO UPDATE SET reference = EXCLUDED.reference`, so pushing one manifest under a
-- second tag *rewrote* the first tag away — the repository then had no route to
-- content it had been serving under the original tag. Only one tag per digest
-- could ever exist.
--
-- A tag is a mutable pointer and a manifest is immutable content named by its own
-- hash; they are different kinds of thing and need different tables. After this,
-- many tags may point at one retained manifest, and repointing a tag never
-- destroys content.
--
-- Forward-only and data-preserving: every existing manifest row is kept, and its
-- `reference` becomes a row in `oci_tags`.

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

-- Backfill from the column being retired. Digest-shaped references were never
-- tags (a tag cannot contain ':'), so they are skipped rather than turned into
-- bogus tag rows.
--
-- Repointing a tag left MULTIPLE rows carrying it (the upsert only rewrote the
-- row it conflicted on), so a tag can appear several times and exactly one of
-- them must win. The pre-migration read path resolved a tag with
-- `ORDER BY created_at DESC LIMIT 1`, so ordering by `created_at DESC` here
-- reproduces precisely what that tag served before — picking any other row would
-- silently roll the tag back to an older image.
--
-- `digest DESC` is the tiebreak, and it is load-bearing: rows written inside one
-- transaction share a `created_at`, and without a total order Postgres is free to
-- return either, so the same dump could migrate to two different results. It only
-- decides cases that were already ambiguous before the migration.
INSERT INTO oci_tags (repository, tag, digest)
SELECT DISTINCT ON (repository, reference) repository, reference, digest
FROM oci_manifests
WHERE reference IS NOT NULL
  AND reference NOT LIKE '%:%'
ORDER BY repository, reference, created_at DESC, digest DESC;

DROP INDEX IF EXISTS idx_manifests_repo_ref;
ALTER TABLE oci_manifests DROP COLUMN reference;

-- Referrers: scope the identity by repository. Two repositories publishing the
-- same referrer for the same subject collided on the old
-- UNIQUE(subject_digest, referrer_digest), and the second insert was silently
-- discarded (ON CONFLICT DO NOTHING), leaving that repository's referrers
-- endpoint empty. Also drop the surrogate `id`, which bought nothing once the
-- natural key is right, and add the FK onto the referring manifest so referrers
-- cannot outlive it.
DELETE FROM oci_referrers r
WHERE NOT EXISTS (
    SELECT 1 FROM oci_manifests m
    WHERE m.repository = r.repository AND m.digest = r.referrer_digest
);

ALTER TABLE oci_referrers DROP CONSTRAINT IF EXISTS oci_referrers_subject_digest_referrer_digest_key;
ALTER TABLE oci_referrers DROP CONSTRAINT IF EXISTS oci_referrers_pkey;
ALTER TABLE oci_referrers DROP COLUMN IF EXISTS id;
ALTER TABLE oci_referrers
    ADD PRIMARY KEY (repository, subject_digest, referrer_digest);
ALTER TABLE oci_referrers
    ADD FOREIGN KEY (repository, referrer_digest)
    REFERENCES oci_manifests(repository, digest) ON DELETE CASCADE;

-- Pending physical blob deletions. A Postgres transaction cannot make an S3
-- delete atomic, so the two are decoupled: dropping the last reference commits a
-- tombstone here and nothing else, then a sweep re-checks the reference count
-- under a digest-scoped advisory lock and removes the bytes. The commit is the
-- only durable decision, so failures are one-sided — a crash or a failed storage
-- call leaves reclaimable bytes with a tombstone still queued, never a committed
-- reference pointing at bytes that are already gone.
CREATE TABLE oci_blob_gc (
    digest       TEXT        PRIMARY KEY,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
