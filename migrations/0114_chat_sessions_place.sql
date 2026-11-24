-- Where a coding-agent session ran: the working folder, the branch and the git repository, as the
-- reporting client read them on the developer's machine (`session.place` on a coding-agent
-- event). Replay evals find a session's repository here, so a person picks sessions and never
-- names a repository.
--
-- Sent only with chat history (the client's content capture), like the session title; a server
-- that keeps no chat history drops it on ingest. Filled once, by the first turn that carries it,
-- including a replayed turn, so re-syncing an older session's history backfills it.
--
-- `repo_host` is the remote's host (`github.com`), `repo_owner`/`repo_name` its `owner/name`, and
-- `repo_root` the working tree's top folder, which turns the session's edit paths repo-relative.
-- No credential from the remote URL is ever sent.
--
-- cold: compatible (chat_sessions is never tiered)

ALTER TABLE chat_sessions
    ADD COLUMN cwd TEXT,
    ADD COLUMN git_branch TEXT,
    ADD COLUMN repo_host TEXT,
    ADD COLUMN repo_owner TEXT,
    ADD COLUMN repo_name TEXT,
    ADD COLUMN repo_root TEXT;
