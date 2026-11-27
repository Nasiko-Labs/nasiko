-- Every branch a coding-agent session reported (`session.place.git_branches`), first seen first.
-- `git_branch` keeps only the first, but a session that switches branches commits on them all, and
-- replay evals search each for the session's commits.
--
-- Unlike the rest of the place, this grows: each turn's list is merged in, keeping first-seen
-- order, so a later turn on a new branch (or a re-synced history) adds it.
--
-- cold: compatible (chat_sessions is never tiered)

ALTER TABLE chat_sessions
    ADD COLUMN git_branches TEXT[] NOT NULL DEFAULT '{}';
