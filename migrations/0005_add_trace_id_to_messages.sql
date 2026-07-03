ALTER TABLE chat_messages ADD COLUMN trace_id TEXT;

CREATE INDEX idx_chat_messages_trace ON chat_messages(trace_id) WHERE trace_id IS NOT NULL;
