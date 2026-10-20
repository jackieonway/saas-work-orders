CREATE TABLE work_order_reminder_intents (
    tenant_id UUID NOT NULL,
    organization_id UUID NOT NULL,
    application_id UUID NOT NULL,
    user_id UUID NOT NULL,
    idempotency_key VARCHAR(128) NOT NULL CHECK (idempotency_key COLLATE "C" ~ '^[A-Za-z0-9._:-]{1,128}$'),
    ticket_id UUID NOT NULL,
    target_organization_id UUID NOT NULL,
    recipient_user_id UUID NOT NULL,
    request_hash CHAR(64) NOT NULL CHECK (request_hash COLLATE "C" ~ '^[0-9a-f]{64}$'),
    request_json TEXT NOT NULL CHECK (octet_length(request_json) BETWEEN 1 AND 65536),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, organization_id, application_id, user_id, idempotency_key)
);

-- An event survives ticket deletion and process restarts. It is neither a
-- publication receipt nor a retry queue, and cannot be redirected or recycled.
CREATE FUNCTION work_order_reminder_intent_immutable() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'work order reminder intents are immutable' USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER work_order_reminder_intent_immutable
BEFORE UPDATE OR DELETE ON work_order_reminder_intents
FOR EACH ROW EXECUTE FUNCTION work_order_reminder_intent_immutable();
