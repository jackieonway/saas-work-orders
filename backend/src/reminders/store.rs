use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::{authorization::ReminderAuthorized, error::ApiError};

use super::dto::{self, Ticket};

const MAX_JSON: i32 = 65_536;

fn database(_error: sqlx::Error) -> ApiError {
    ApiError::database_unavailable()
}

pub(super) async fn claim(
    pool: &PgPool,
    auth: &ReminderAuthorized,
    id: Uuid,
    key: &str,
) -> Result<String, ApiError> {
    auth.assert_fresh()?;
    let read = auth.read();
    let mut transaction = crate::telemetry::begin(pool).await.map_err(database)?;
    auth.assert_fresh()?;
    let mut select = QueryBuilder::<Postgres>::new("SELECT id,number,organization_id,owner_user_id,assignee_user_id FROM public.work_orders WHERE ");
    read.append_scope(&mut select);
    let row = select
        .push(" AND id=")
        .push_bind(id)
        .push(" FOR SHARE")
        .build()
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database)?
        .ok_or_else(ApiError::not_found)?;
    auth.assert_fresh()?;
    let ticket = Ticket {
        id: row.try_get("id").map_err(database)?,
        number: row.try_get("number").map_err(database)?,
        organization: row.try_get("organization_id").map_err(database)?,
        owner: row.try_get("owner_user_id").map_err(database)?,
        assignee: row.try_get("assignee_user_id").map_err(database)?,
    };
    auth.assert_recipient(ticket.organization, ticket.recipient())?;
    let candidate = dto::payload(&ticket, key);
    let json = serde_json::to_string(&candidate).map_err(|_| ApiError::database_unavailable())?;
    let hash = format!("{:x}", Sha256::digest(json.as_bytes()));
    sqlx::query("INSERT INTO public.work_order_reminder_intents(tenant_id,organization_id,application_id,user_id,idempotency_key,ticket_id,target_organization_id,recipient_user_id,request_hash,request_json) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(tenant_id,organization_id,application_id,user_id,idempotency_key) DO NOTHING")
        .bind(read.tenant_id()).bind(read.organization_id()).bind(auth.application_id()).bind(read.user_id()).bind(key)
        .bind(ticket.id).bind(ticket.organization).bind(ticket.recipient()).bind(&hash).bind(&json)
        .execute(&mut *transaction).await.map_err(database)?;
    auth.assert_fresh()?;
    let intent = sqlx::query("SELECT tenant_id,organization_id,application_id,user_id,idempotency_key,ticket_id,target_organization_id,recipient_user_id,request_hash,CASE WHEN octet_length(request_json)<=$6 THEN request_json END AS request_json FROM public.work_order_reminder_intents WHERE tenant_id=$1 AND organization_id=$2 AND application_id=$3 AND user_id=$4 AND idempotency_key=$5 FOR SHARE")
        .bind(read.tenant_id()).bind(read.organization_id()).bind(auth.application_id()).bind(read.user_id()).bind(key).bind(MAX_JSON)
        .fetch_one(&mut *transaction).await.map_err(database)?;
    auth.assert_fresh()?;
    for (field, expected) in [
        ("tenant_id", read.tenant_id()),
        ("organization_id", read.organization_id()),
        ("application_id", auth.application_id()),
        ("user_id", read.user_id()),
    ] {
        if intent.try_get::<Uuid, _>(field).map_err(database)? != expected {
            return Err(ApiError::database_unavailable());
        }
    }
    if intent
        .try_get::<String, _>("idempotency_key")
        .map_err(database)?
        != key
    {
        return Err(ApiError::database_unavailable());
    }
    let stored_json = intent
        .try_get::<Option<String>, _>("request_json")
        .map_err(database)?
        .ok_or_else(ApiError::database_unavailable)?;
    let stored_hash: String = intent.try_get("request_hash").map_err(database)?;
    if stored_hash != format!("{:x}", Sha256::digest(stored_json.as_bytes())) {
        return Err(ApiError::database_unavailable());
    }
    let old_ticket: Uuid = intent.try_get("ticket_id").map_err(database)?;
    let old_org: Uuid = intent.try_get("target_organization_id").map_err(database)?;
    let old_recipient: Uuid = intent.try_get("recipient_user_id").map_err(database)?;
    let stored = dto::stored_payload(&stored_json, key, old_ticket, old_org, old_recipient)?;
    if old_ticket != ticket.id
        || old_org != ticket.organization
        || old_recipient != ticket.recipient()
        || stored != candidate
        || stored_json != json
        || stored_hash != hash
    {
        return Err(super::conflict());
    }
    auth.assert_fresh()?;
    transaction
        .commit()
        .await
        .map_err(|_| super::preparation_unknown())?;
    // The exact persisted bytes are returned. No reconstruction or local receipt
    // can replace the platform's own durable publication result.
    Ok(stored_json)
}
