ALTER TABLE chat_messages
    ADD COLUMN external_turn_id TEXT;

ALTER TABLE chat_messages
    ADD CONSTRAINT chat_messages_external_turn_role_uniq
    UNIQUE (session_id, external_turn_id, role);
