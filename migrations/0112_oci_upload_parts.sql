-- A chunked upload's bytes move out of server memory into the blob store, one object per chunk
-- (`blobs/uploads/<upload>/<part>`), so a PATCH or the closing PUT can land on any replica.
-- The session row records the chunks' part ids in commit order; the closing PUT reads them back
-- to assemble the blob, and cancel and the startup sweep remove them.
ALTER TABLE oci_uploads ADD COLUMN part_ids UUID[] NOT NULL DEFAULT '{}';
