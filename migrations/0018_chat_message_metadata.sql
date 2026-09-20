ALTER TABLE chat_messages
    ADD COLUMN metadata JSONB;

ALTER TABLE chat_messages
    ADD CONSTRAINT chat_messages_metadata_object
    CHECK (metadata IS NULL OR jsonb_typeof(metadata) = 'object');
