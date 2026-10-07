CREATE TABLE work_order_counters (
    tenant_id UUID PRIMARY KEY,
    next_number BIGINT NOT NULL CHECK (next_number > 0)
);

CREATE TABLE work_orders (
    id UUID PRIMARY KEY,
    number VARCHAR(40) NOT NULL,
    tenant_id UUID NOT NULL,
    organization_id UUID NOT NULL,
    organization_name VARCHAR(100) NOT NULL,
    owner_user_id UUID NOT NULL,
    owner_display_name VARCHAR(100) NOT NULL,
    assignee_user_id UUID,
    assignee_display_name VARCHAR(100),
    title VARCHAR(200) NOT NULL CHECK (length(trim(title)) > 0),
    description TEXT NOT NULL DEFAULT '' CHECK (length(description) <= 8192),
    priority VARCHAR(12) NOT NULL DEFAULT 'NORMAL' CHECK (priority IN ('LOW','NORMAL','HIGH')),
    status VARCHAR(20) NOT NULL DEFAULT 'OPEN' CHECK (status IN ('OPEN','IN_PROGRESS','RESOLVED')),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    UNIQUE (tenant_id, number),
    CHECK ((assignee_user_id IS NULL) = (assignee_display_name IS NULL))
);

CREATE INDEX work_orders_tenant_org_active ON work_orders(tenant_id,organization_id,created_at DESC,id) WHERE deleted_at IS NULL;
CREATE INDEX work_orders_tenant_owner_active ON work_orders(tenant_id,owner_user_id,created_at DESC,id) WHERE deleted_at IS NULL;

CREATE FUNCTION work_order_identity_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.id,NEW.number,NEW.tenant_id,NEW.organization_id,NEW.organization_name,NEW.owner_user_id,NEW.owner_display_name,NEW.created_at)
       IS DISTINCT FROM ROW(OLD.id,OLD.number,OLD.tenant_id,OLD.organization_id,OLD.organization_name,OLD.owner_user_id,OLD.owner_display_name,OLD.created_at) THEN
        RAISE EXCEPTION 'work order identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER work_order_identity_immutable BEFORE UPDATE ON work_orders FOR EACH ROW EXECUTE FUNCTION work_order_identity_immutable();
