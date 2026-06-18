-- Change oci_manifests.content from JSONB to TEXT so we store the exact bytes
-- the client uploaded. JSONB normalises key order and whitespace, causing the
-- sha256 digest we computed at upload time to no longer match the served body.
ALTER TABLE oci_manifests ALTER COLUMN content TYPE TEXT;
