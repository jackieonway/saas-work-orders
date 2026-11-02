-- Fresh installation baseline. Existing installations must be recreated; no historical adoption.
-- PostgreSQL 17+, application-generated UUIDv7, SSO-only console.
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;

CREATE FUNCTION public.work_order_identity_immutable() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF ROW(NEW.id,NEW.number,NEW.tenant_id,NEW.organization_id,NEW.organization_name,NEW.owner_user_id,NEW.owner_display_name,NEW.created_at)
       IS DISTINCT FROM ROW(OLD.id,OLD.number,OLD.tenant_id,OLD.organization_id,OLD.organization_name,OLD.owner_user_id,OLD.owner_display_name,OLD.created_at) THEN
        RAISE EXCEPTION 'work order identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION public.work_order_reminder_intent_immutable() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION 'work order reminder intents are immutable' USING ERRCODE = '23514';
END;
$$;

CREATE FUNCTION public.work_orders_schema_fingerprint() RETURNS text
    LANGUAGE sql STABLE
    SET search_path TO 'pg_catalog', 'public'
    AS $$
SELECT md5(COALESCE(string_agg(item,E'\n' ORDER BY item),'')) FROM (
 SELECT 'relation|'||c.relname||'|'||c.relkind::text||'|'||c.relpersistence::text||'|'||c.relrowsecurity AS item FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','v','m','S','p') AND c.relname<>'_sqlx_migrations'
 UNION ALL SELECT 'column|'||c.relname||'|'||a.attname||'|'||format_type(a.atttypid,a.atttypmod)||'|'||a.attnotnull||'|'||COALESCE(pg_get_expr(d.adbin,d.adrelid),'') FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE n.nspname='public' AND a.attnum>0 AND NOT a.attisdropped AND c.relkind IN ('r','p') AND c.relname<>'_sqlx_migrations'
 UNION ALL SELECT 'constraint|'||c.relname||'|'||k.conname||'|'||pg_get_constraintdef(k.oid,true) FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname<>'_sqlx_migrations'
 UNION ALL SELECT 'index|'||tablename||'|'||indexname||'|'||indexdef FROM pg_indexes WHERE schemaname='public' AND tablename<>'_sqlx_migrations'
 UNION ALL SELECT 'trigger|'||c.relname||'|'||t.tgname||'|'||t.tgenabled::text||'|'||pg_get_triggerdef(t.oid,true) FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND NOT t.tgisinternal
 UNION ALL SELECT 'function|'||p.proname||'|'||pg_get_function_identity_arguments(p.oid)||'|'||pg_get_functiondef(p.oid) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public'
) schema_items
$$;

SET default_table_access_method = heap;

CREATE TABLE public._work_orders_schema_contract (
    singleton boolean DEFAULT true NOT NULL,
    fingerprint text NOT NULL,
    CONSTRAINT _work_orders_schema_contract_fingerprint_check CHECK ((length(fingerprint) = 32)),
    CONSTRAINT _work_orders_schema_contract_singleton_check CHECK (singleton)
);

CREATE TABLE public.work_order_counters (
    tenant_id uuid NOT NULL,
    next_number bigint NOT NULL,
    CONSTRAINT work_order_counters_next_number_check CHECK ((next_number > 0)),
    CONSTRAINT work_order_counters_tenant_id_uuid_v7 CHECK (((tenant_id IS NULL) OR (((get_byte(uuid_send(tenant_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(tenant_id), 8) & 192) = 128))))
);

CREATE TABLE public.work_order_create_idempotency (
    tenant_id uuid NOT NULL,
    organization_id uuid NOT NULL,
    user_id uuid NOT NULL,
    action text NOT NULL,
    idempotency_key character varying(128) NOT NULL,
    request_hash character(64) NOT NULL,
    ticket_id uuid,
    response_json text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT work_order_create_idempotency_action_check CHECK ((action = 'create'::text)),
    CONSTRAINT work_order_create_idempotency_check CHECK (((ticket_id IS NULL) = (response_json IS NULL))),
    CONSTRAINT work_order_create_idempotency_idempotency_key_check CHECK ((((idempotency_key)::text COLLATE "C") ~ '^[A-Za-z0-9._:-]{1,128}$'::text)),
    CONSTRAINT work_order_create_idempotency_organization_id_uuid_v7 CHECK (((organization_id IS NULL) OR (((get_byte(uuid_send(organization_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(organization_id), 8) & 192) = 128)))),
    CONSTRAINT work_order_create_idempotency_request_hash_check CHECK (((request_hash COLLATE "C") ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT work_order_create_idempotency_tenant_id_uuid_v7 CHECK (((tenant_id IS NULL) OR (((get_byte(uuid_send(tenant_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(tenant_id), 8) & 192) = 128)))),
    CONSTRAINT work_order_create_idempotency_ticket_id_uuid_v7 CHECK (((ticket_id IS NULL) OR (((get_byte(uuid_send(ticket_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(ticket_id), 8) & 192) = 128)))),
    CONSTRAINT work_order_create_idempotency_user_id_uuid_v7 CHECK (((user_id IS NULL) OR (((get_byte(uuid_send(user_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(user_id), 8) & 192) = 128))))
);

CREATE TABLE public.work_order_reminder_intents (
    tenant_id uuid NOT NULL,
    organization_id uuid NOT NULL,
    application_id uuid NOT NULL,
    user_id uuid NOT NULL,
    idempotency_key character varying(128) NOT NULL,
    ticket_id uuid NOT NULL,
    target_organization_id uuid NOT NULL,
    recipient_user_id uuid NOT NULL,
    request_hash character(64) NOT NULL,
    request_json text NOT NULL,
    token_digest character(64) NOT NULL,
    source_binding character(64) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT work_order_reminder_intents_application_id_uuid_v7 CHECK (((application_id IS NULL) OR (((get_byte(uuid_send(application_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(application_id), 8) & 192) = 128)))),
    CONSTRAINT work_order_reminder_intents_idempotency_key_check CHECK ((((idempotency_key)::text COLLATE "C") ~ '^[A-Za-z0-9._:-]{1,128}$'::text)),
    CONSTRAINT work_order_reminder_intents_organization_id_uuid_v7 CHECK (((organization_id IS NULL) OR (((get_byte(uuid_send(organization_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(organization_id), 8) & 192) = 128)))),
    CONSTRAINT work_order_reminder_intents_recipient_user_id_uuid_v7 CHECK (((recipient_user_id IS NULL) OR (((get_byte(uuid_send(recipient_user_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(recipient_user_id), 8) & 192) = 128)))),
    CONSTRAINT work_order_reminder_intents_request_hash_check CHECK (((request_hash COLLATE "C") ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT work_order_reminder_intents_token_digest_check CHECK (((token_digest COLLATE "C") ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT work_order_reminder_intents_source_binding_check CHECK (((source_binding COLLATE "C") ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT work_order_reminder_intents_request_json_check CHECK (((octet_length(request_json) >= 1) AND (octet_length(request_json) <= 65536))),
    CONSTRAINT work_order_reminder_intents_target_organization_id_uuid_v7 CHECK (((target_organization_id IS NULL) OR (((get_byte(uuid_send(target_organization_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(target_organization_id), 8) & 192) = 128)))),
    CONSTRAINT work_order_reminder_intents_tenant_id_uuid_v7 CHECK (((tenant_id IS NULL) OR (((get_byte(uuid_send(tenant_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(tenant_id), 8) & 192) = 128)))),
    CONSTRAINT work_order_reminder_intents_ticket_id_uuid_v7 CHECK (((ticket_id IS NULL) OR (((get_byte(uuid_send(ticket_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(ticket_id), 8) & 192) = 128)))),
    CONSTRAINT work_order_reminder_intents_user_id_uuid_v7 CHECK (((user_id IS NULL) OR (((get_byte(uuid_send(user_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(user_id), 8) & 192) = 128))))
);

CREATE TABLE public.work_orders (
    id uuid NOT NULL,
    number character varying(40) NOT NULL,
    tenant_id uuid NOT NULL,
    organization_id uuid NOT NULL,
    organization_name character varying(100) NOT NULL,
    owner_user_id uuid NOT NULL,
    owner_display_name character varying(100) NOT NULL,
    assignee_user_id uuid,
    assignee_display_name character varying(100),
    title character varying(200) NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    priority character varying(12) DEFAULT 'NORMAL'::character varying NOT NULL,
    status character varying(20) DEFAULT 'OPEN'::character varying NOT NULL,
    version integer DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT work_orders_assignee_user_id_uuid_v7 CHECK (((assignee_user_id IS NULL) OR (((get_byte(uuid_send(assignee_user_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(assignee_user_id), 8) & 192) = 128)))),
    CONSTRAINT work_orders_check CHECK (((assignee_user_id IS NULL) = (assignee_display_name IS NULL))),
    CONSTRAINT work_orders_description_check CHECK ((length(description) <= 8192)),
    CONSTRAINT work_orders_id_uuid_v7 CHECK (((id IS NULL) OR (((get_byte(uuid_send(id), 6) >> 4) = 7) AND ((get_byte(uuid_send(id), 8) & 192) = 128)))),
    CONSTRAINT work_orders_organization_id_uuid_v7 CHECK (((organization_id IS NULL) OR (((get_byte(uuid_send(organization_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(organization_id), 8) & 192) = 128)))),
    CONSTRAINT work_orders_owner_user_id_uuid_v7 CHECK (((owner_user_id IS NULL) OR (((get_byte(uuid_send(owner_user_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(owner_user_id), 8) & 192) = 128)))),
    CONSTRAINT work_orders_priority_check CHECK (((priority)::text = ANY ((ARRAY['LOW'::character varying, 'NORMAL'::character varying, 'HIGH'::character varying])::text[]))),
    CONSTRAINT work_orders_status_check CHECK (((status)::text = ANY ((ARRAY['OPEN'::character varying, 'IN_PROGRESS'::character varying, 'RESOLVED'::character varying])::text[]))),
    CONSTRAINT work_orders_tenant_id_uuid_v7 CHECK (((tenant_id IS NULL) OR (((get_byte(uuid_send(tenant_id), 6) >> 4) = 7) AND ((get_byte(uuid_send(tenant_id), 8) & 192) = 128)))),
    CONSTRAINT work_orders_title_check CHECK ((length(TRIM(BOTH FROM title)) > 0)),
    CONSTRAINT work_orders_version_check CHECK ((version > 0))
);

ALTER TABLE ONLY public._work_orders_schema_contract
    ADD CONSTRAINT _work_orders_schema_contract_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY public.work_order_counters
    ADD CONSTRAINT work_order_counters_pkey PRIMARY KEY (tenant_id);

ALTER TABLE ONLY public.work_order_create_idempotency
    ADD CONSTRAINT work_order_create_idempotency_pkey PRIMARY KEY (tenant_id, organization_id, user_id, action, idempotency_key);

ALTER TABLE ONLY public.work_order_reminder_intents
    ADD CONSTRAINT work_order_reminder_intents_pkey PRIMARY KEY (tenant_id, organization_id, application_id, user_id, idempotency_key);

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_tenant_id_number_key UNIQUE (tenant_id, number);

CREATE INDEX work_orders_tenant_org_active ON public.work_orders USING btree (tenant_id, organization_id, created_at DESC, id) WHERE (deleted_at IS NULL);

CREATE INDEX work_orders_tenant_owner_active ON public.work_orders USING btree (tenant_id, owner_user_id, created_at DESC, id) WHERE (deleted_at IS NULL);

CREATE TRIGGER work_order_identity_immutable BEFORE UPDATE ON public.work_orders FOR EACH ROW EXECUTE FUNCTION public.work_order_identity_immutable();

CREATE TRIGGER work_order_reminder_intent_immutable BEFORE DELETE OR UPDATE ON public.work_order_reminder_intents FOR EACH ROW EXECUTE FUNCTION public.work_order_reminder_intent_immutable();

ALTER TABLE ONLY public.work_order_create_idempotency
    ADD CONSTRAINT work_order_create_idempotency_ticket_id_fkey FOREIGN KEY (ticket_id) REFERENCES public.work_orders(id);

INSERT INTO public._work_orders_schema_contract(singleton,fingerprint) VALUES(true,public.work_orders_schema_fingerprint());
SET check_function_bodies = true;
SELECT pg_catalog.set_config('search_path','public,pg_catalog,pg_temp',false);
