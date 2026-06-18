-- Seed default admin user for OSS mode (SingleUserAuth returns nil UUID)
INSERT INTO users (id, username, email, is_superuser)
VALUES ('00000000-0000-0000-0000-000000000000', 'admin', 'admin@localhost', true)
ON CONFLICT (id) DO NOTHING;
