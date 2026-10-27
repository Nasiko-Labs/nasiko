-- OV-2 (ui/docs/requirements/overview.md): what people did about an Overview problem — who is on
-- it, a snooze, "handled". Problems themselves are never stored: the estate computes them on every
-- read. Only these decisions are kept, keyed by the problem's stable id (`<rule>:<entity id>`), and
-- shared by everyone who can see the problem, so a team sees who is on what.
CREATE TABLE overview_problem_state (
    problem_id    TEXT PRIMARY KEY,
    owner_id      UUID REFERENCES users(id) ON DELETE SET NULL,
    snoozed_until TIMESTAMPTZ,
    -- "Handled" hides a problem for the rest of its UTC day; the estate shows it again the next
    -- day if its metric is still behind, so a handled mark can never hide a live problem for long.
    handled_at    TIMESTAMPTZ,
    updated_by    UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
