-- Allow files to be uploaded before a message exists (two-phase upload).
ALTER TABLE chat_message_files ALTER COLUMN message_id DROP NOT NULL;
