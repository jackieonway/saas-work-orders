-- UUID identity epoch: application-generated RFC UUIDv7 on PostgreSQL 17+.
-- Historical migration bytes are unchanged. Existing non-v7 data is rejected;
-- remap/export into a new isolated database before starting this version.
ALTER TABLE work_order_counters ADD CONSTRAINT work_order_counters_tenant_id_uuid_v7
    CHECK (tenant_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(tenant_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(tenant_id), 8) & 192) = 128
    ));
ALTER TABLE work_orders ADD CONSTRAINT work_orders_id_uuid_v7
    CHECK (id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(id), 8) & 192) = 128
    ));
ALTER TABLE work_orders ADD CONSTRAINT work_orders_tenant_id_uuid_v7
    CHECK (tenant_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(tenant_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(tenant_id), 8) & 192) = 128
    ));
ALTER TABLE work_orders ADD CONSTRAINT work_orders_organization_id_uuid_v7
    CHECK (organization_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(organization_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(organization_id), 8) & 192) = 128
    ));
ALTER TABLE work_orders ADD CONSTRAINT work_orders_owner_user_id_uuid_v7
    CHECK (owner_user_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(owner_user_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(owner_user_id), 8) & 192) = 128
    ));
ALTER TABLE work_orders ADD CONSTRAINT work_orders_assignee_user_id_uuid_v7
    CHECK (assignee_user_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(assignee_user_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(assignee_user_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_create_idempotency ADD CONSTRAINT work_order_create_idempotency_tenant_id_uuid_v7
    CHECK (tenant_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(tenant_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(tenant_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_create_idempotency ADD CONSTRAINT work_order_create_idempotency_organization_id_uuid_v7
    CHECK (organization_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(organization_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(organization_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_create_idempotency ADD CONSTRAINT work_order_create_idempotency_user_id_uuid_v7
    CHECK (user_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(user_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(user_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_create_idempotency ADD CONSTRAINT work_order_create_idempotency_ticket_id_uuid_v7
    CHECK (ticket_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(ticket_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(ticket_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_reminder_intents ADD CONSTRAINT work_order_reminder_intents_tenant_id_uuid_v7
    CHECK (tenant_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(tenant_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(tenant_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_reminder_intents ADD CONSTRAINT work_order_reminder_intents_organization_id_uuid_v7
    CHECK (organization_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(organization_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(organization_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_reminder_intents ADD CONSTRAINT work_order_reminder_intents_application_id_uuid_v7
    CHECK (application_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(application_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(application_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_reminder_intents ADD CONSTRAINT work_order_reminder_intents_user_id_uuid_v7
    CHECK (user_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(user_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(user_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_reminder_intents ADD CONSTRAINT work_order_reminder_intents_ticket_id_uuid_v7
    CHECK (ticket_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(ticket_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(ticket_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_reminder_intents ADD CONSTRAINT work_order_reminder_intents_target_organization_id_uuid_v7
    CHECK (target_organization_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(target_organization_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(target_organization_id), 8) & 192) = 128
    ));
ALTER TABLE work_order_reminder_intents ADD CONSTRAINT work_order_reminder_intents_recipient_user_id_uuid_v7
    CHECK (recipient_user_id IS NULL OR (
        (pg_catalog.get_byte(pg_catalog.uuid_send(recipient_user_id), 6) >> 4) = 7
        AND (pg_catalog.get_byte(pg_catalog.uuid_send(recipient_user_id), 8) & 192) = 128
    ));
