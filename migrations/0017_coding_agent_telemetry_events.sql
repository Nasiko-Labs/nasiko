CREATE TABLE coding_agent_telemetry_events (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    event_id TEXT NOT NULL,
    payload JSONB NOT NULL,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    agent_name TEXT NOT NULL,
    source_agent_id TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES chat_sessions(session_id) ON DELETE RESTRICT,
    source_session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    captured_at TIMESTAMPTZ NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    otlp_state TEXT NOT NULL DEFAULT 'pending'
        CHECK (otlp_state IN ('pending', 'processing', 'delivered', 'failed')),
    otlp_attempts INTEGER NOT NULL DEFAULT 0 CHECK (otlp_attempts >= 0),
    otlp_last_attempt_at TIMESTAMPTZ,
    otlp_next_attempt_at TIMESTAMPTZ,
    otlp_delivered_at TIMESTAMPTZ,
    otlp_trace_delivered_at TIMESTAMPTZ,
    otlp_log_delivered_at TIMESTAMPTZ,
    otlp_last_error TEXT,
    otlp_last_error_at TIMESTAMPTZ,
    otlp_claim_id UUID,
    PRIMARY KEY (user_id, event_id)
);

CREATE INDEX idx_coding_agent_telemetry_otlp_ready
    ON coding_agent_telemetry_events (otlp_next_attempt_at, received_at)
    WHERE otlp_state IN ('pending', 'failed');
CREATE INDEX idx_coding_agent_telemetry_otlp_processing
    ON coding_agent_telemetry_events (otlp_last_attempt_at)
    WHERE otlp_state = 'processing';
CREATE INDEX idx_coding_agent_telemetry_session
    ON coding_agent_telemetry_events (session_id, received_at);

-- Receipt identity and payload are immutable. A later outbox worker may only
-- advance the explicitly mutable OTLP delivery columns.
CREATE FUNCTION preserve_coding_agent_telemetry_receipt() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.user_id, NEW.event_id, NEW.payload, NEW.agent_id, NEW.agent_name,
        NEW.source_agent_id, NEW.session_id, NEW.source_session_id, NEW.turn_id,
        NEW.captured_at, NEW.received_at)
       IS DISTINCT FROM
       (OLD.user_id, OLD.event_id, OLD.payload, OLD.agent_id, OLD.agent_name,
        OLD.source_agent_id, OLD.session_id, OLD.source_session_id, OLD.turn_id,
        OLD.captured_at, OLD.received_at) THEN
        RAISE EXCEPTION 'coding-agent telemetry receipts are immutable';
    END IF;
    RETURN NEW;
END; $$;

CREATE TRIGGER trg_coding_agent_telemetry_receipt_immutable
    BEFORE UPDATE ON coding_agent_telemetry_events
    FOR EACH ROW EXECUTE FUNCTION preserve_coding_agent_telemetry_receipt();
