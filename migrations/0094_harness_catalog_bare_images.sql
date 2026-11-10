-- The launch harnesses were seeded with qualified image names (`nasiko/harness-<id>:<tag>`),
-- which `sessions::catalog::qualified_image` keeps verbatim, so `HARNESS_IMAGE_PREFIX` never
-- reached them and a deployment pulling from a mirror still pulled from Docker Hub. Store the
-- catalog shorthand instead: a bare `<id>:<tag>` gets the prefix at session time, and the
-- default prefix (`nasiko/harness-`) resolves to the same references as before.
UPDATE harness_catalog
SET image = substr(image, length('nasiko/harness-') + 1)
WHERE image LIKE 'nasiko/harness-%';
