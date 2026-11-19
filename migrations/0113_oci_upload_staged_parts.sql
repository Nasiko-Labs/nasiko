-- A chunk is recorded as staged before its object is written, and moved to `part_ids` when its
-- offset commits. A replica that dies between the two leaves the id here, so removing the session
-- (completion, cancel, the abandoned-upload sweep) still finds and deletes the object, which the
-- blob store offers no listing to find otherwise.
ALTER TABLE oci_uploads ADD COLUMN staged_part_ids UUID[] NOT NULL DEFAULT '{}';
