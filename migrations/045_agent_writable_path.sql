-- Container-side mount target for the `--writable` persistent volume
-- (`--writable-path`). NULL = the default `/workspace`. Only meaningful when
-- `writable` is true; carried through update/rollback/restart exactly like
-- `writable` (migration 044) so a redeploy never silently moves the mount.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS writable_path TEXT;
