CREATE TABLE work_order_create_idempotency (
    tenant_id UUID NOT NULL,
    organization_id UUID NOT NULL,
    user_id UUID NOT NULL,
    action TEXT NOT NULL CHECK (action = 'create'),
    idempotency_key VARCHAR(128) NOT NULL CHECK (idempotency_key COLLATE "C" ~ '^[A-Za-z0-9._:-]{1,128}$'),
    request_hash CHAR(64) NOT NULL CHECK (request_hash COLLATE "C" ~ '^[0-9a-f]{64}$'),
    ticket_id UUID REFERENCES work_orders(id),
    response_json TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, organization_id, user_id, action, idempotency_key),
    CHECK ((ticket_id IS NULL) = (response_json IS NULL))
);
