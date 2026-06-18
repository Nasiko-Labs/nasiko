CREATE EXTENSION IF NOT EXISTS vector;

ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS embedding vector(1536);

CREATE OR REPLACE FUNCTION immutable_array_to_string(arr text[], sep text)
RETURNS text LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT array_to_string(arr, sep)
$$;

ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS search_vector tsvector
    GENERATED ALWAYS AS (
        to_tsvector('pg_catalog.english'::regconfig,
            coalesce(name, '') || ' ' ||
            coalesce(description, '') || ' ' ||
            coalesce(immutable_array_to_string(tags, ' '), '')
        )
    ) STORED;

CREATE INDEX ON artifacts USING GIN(search_vector);
CREATE INDEX ON artifacts USING ivfflat(embedding vector_cosine_ops) WITH (lists = 100);
