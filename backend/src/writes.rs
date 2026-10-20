//! Work-order mutations with external member preflight before transactions.
//! No network directory call runs while holding a business connection or lock.

mod dto;

use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::{
    authorization::{Action, Authorized, Authorizer},
    error::ApiError,
    reads::{Ticket, RECORD_COLUMNS},
};

use dto::Patch;

const RECEIPT_DOMAIN: &str = "tenant_id=$1 AND organization_id=$2 AND user_id=$3 \
    AND action='create' AND idempotency_key=$4";
const MAX_RECEIPT_BYTES: usize = 128 * 1024;

fn database_error(_error: sqlx::Error) -> ApiError {
    ApiError::database_unavailable()
}

fn version_conflict() -> ApiError {
    ApiError::new(409, "VERSION_CONFLICT", "工单已被修改，请刷新后重试")
}

fn idempotency_conflict() -> ApiError {
    ApiError::new(
        409,
        "IDEMPOTENCY_CONFLICT",
        "创建幂等键已用于不同正文，请使用原始请求重试",
    )
}

fn commit_unknown(create: bool) -> ApiError {
    if create {
        ApiError::new(
            503,
            "CREATE_RESULT_UNKNOWN",
            "创建结果尚未确认，请使用相同幂等键和正文重试",
        )
    } else {
        ApiError::new(
            503,
            "WRITE_RESULT_UNKNOWN",
            "写入结果尚未确认，请刷新工单核验当前版本",
        )
    }
}

fn receipt_response(
    auth: &Authorized,
    ticket_id: Uuid,
    response: &str,
) -> Result<Ticket, ApiError> {
    if response.len() > MAX_RECEIPT_BYTES {
        return Err(ApiError::database_unavailable());
    }
    let value: serde_json::Value =
        serde_json::from_str(response).map_err(|_| ApiError::database_unavailable())?;
    for (field, expected) in [
        ("id", ticket_id),
        ("tenantId", auth.tenant_id()),
        ("organizationId", auth.organization_id()),
        ("ownerUserId", auth.user_id()),
    ] {
        if value.get(field).and_then(serde_json::Value::as_str)
            != Some(expected.to_string().as_str())
        {
            return Err(ApiError::database_unavailable());
        }
    }
    serde_json::from_value(value).map_err(|_| ApiError::database_unavailable())
}

/// A committed receipt is creation history, not a new assignment. Read it
/// without a lock and release the connection before any directory request.
async fn completed_receipt(
    pool: &PgPool,
    auth: &Authorized,
    key: &str,
    request_hash: &str,
) -> Result<Option<Ticket>, ApiError> {
    let sql = format!(
        "SELECT request_hash,ticket_id, \
         CASE WHEN pg_catalog.octet_length(response_json)<={MAX_RECEIPT_BYTES} THEN response_json END AS response_json, \
         response_json IS NOT NULL AS response_present \
         FROM public.work_order_create_idempotency WHERE {RECEIPT_DOMAIN}"
    );
    let row = sqlx::query(&sql)
        .bind(auth.tenant_id())
        .bind(auth.organization_id())
        .bind(auth.user_id())
        .bind(key)
        .fetch_optional(pool)
        .await
        .map_err(database_error)?;
    auth.assert_fresh()?;
    let Some(row) = row else { return Ok(None) };
    let hash: String = row.try_get("request_hash").map_err(database_error)?;
    if hash != request_hash {
        return Err(idempotency_conflict());
    }
    let present: bool = row.try_get("response_present").map_err(database_error)?;
    let ticket: Option<Uuid> = row.try_get("ticket_id").map_err(database_error)?;
    if !present {
        if ticket.is_some() {
            return Err(ApiError::database_unavailable());
        }
        return Ok(None);
    }
    let response: Option<String> = row.try_get("response_json").map_err(database_error)?;
    receipt_response(
        auth,
        ticket.ok_or_else(ApiError::database_unavailable)?,
        response
            .as_deref()
            .ok_or_else(ApiError::database_unavailable)?,
    )
    .map(Some)
}

pub(crate) async fn create(
    pool: &PgPool,
    auth: &Authorized,
    idempotency_key: &str,
    body: &[u8],
    authorizer: &Authorizer,
    token: &str,
    request_id: &str,
) -> Result<Ticket, ApiError> {
    auth.require_action(Action::Create)?;
    auth.assert_fresh()?;
    auth.require_organization(auth.organization_id())?;
    dto::idempotency_key(idempotency_key)?;
    let body = dto::create(body)?;
    let request_hash = dto::fingerprint(&body)?;
    let assignee = body
        .assignee_user_id
        .as_deref()
        .map(dto::canonical_uuid)
        .transpose()?;
    let verified = if let Some(user) = assignee {
        if let Some(ticket) = completed_receipt(pool, auth, idempotency_key, &request_hash).await? {
            auth.assert_fresh()?;
            return Ok(ticket);
        }
        Some(
            authorizer
                .member(
                    auth,
                    token,
                    auth.organization_id(),
                    user,
                    Action::Create,
                    request_id,
                )
                .await?,
        )
    } else {
        None
    };

    let mut transaction = crate::telemetry::begin(pool)
        .await
        .map_err(database_error)?;
    auth.assert_fresh()?;
    // An uncommitted unique-key conflict waits within PostgreSQL's configured
    // lock/statement budgets. No receipt placeholder is ever committed alone.
    sqlx::query(
        "INSERT INTO public.work_order_create_idempotency \
         (tenant_id,organization_id,user_id,action,idempotency_key,request_hash) \
         VALUES($1,$2,$3,'create',$4,$5) \
         ON CONFLICT(tenant_id,organization_id,user_id,action,idempotency_key) DO NOTHING",
    )
    .bind(auth.tenant_id())
    .bind(auth.organization_id())
    .bind(auth.user_id())
    .bind(idempotency_key)
    .bind(&request_hash)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    auth.assert_fresh()?;
    let receipt_sql = format!(
        "SELECT request_hash,ticket_id, \
         CASE WHEN pg_catalog.octet_length(response_json)<={} THEN response_json END AS response_json, \
         response_json IS NOT NULL AS response_present \
         FROM public.work_order_create_idempotency WHERE {RECEIPT_DOMAIN} FOR UPDATE",
        MAX_RECEIPT_BYTES
    );
    let receipt = sqlx::query(&receipt_sql)
        .bind(auth.tenant_id())
        .bind(auth.organization_id())
        .bind(auth.user_id())
        .bind(idempotency_key)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
    auth.assert_fresh()?;
    auth.require_organization(auth.organization_id())?;
    let stored_hash: String = receipt.try_get("request_hash").map_err(database_error)?;
    if stored_hash != request_hash {
        return Err(idempotency_conflict());
    }
    let ticket_id: Option<Uuid> = receipt.try_get("ticket_id").map_err(database_error)?;
    let response: Option<String> = receipt.try_get("response_json").map_err(database_error)?;
    let response_present: bool = receipt
        .try_get("response_present")
        .map_err(database_error)?;
    if response_present {
        let ticket = receipt_response(
            auth,
            ticket_id.ok_or_else(ApiError::database_unavailable)?,
            response
                .as_deref()
                .ok_or_else(ApiError::database_unavailable)?,
        )?;
        auth.assert_fresh()?;
        transaction
            .commit()
            .await
            .map_err(|_| commit_unknown(true))?;
        auth.assert_fresh().map_err(|_| commit_unknown(true))?;
        return Ok(ticket);
    }
    if ticket_id.is_some() {
        return Err(ApiError::database_unavailable());
    }
    // A racing completed receipt above wins before an expired member witness:
    // returning history never installs a new assignment. New inserts require
    // the original, non-renewable preflight deadline throughout the transaction.
    let assert_assignment = || {
        auth.assert_fresh()?;
        if let (Some(member), Some(user)) = (&verified, assignee) {
            member.assert_for(auth, auth.organization_id(), user, Action::Create)?;
        }
        Ok::<(), ApiError>(())
    };
    assert_assignment()?;

    let number: i64 = sqlx::query_scalar(
        "INSERT INTO public.work_order_counters(tenant_id,next_number) VALUES($1,1) \
         ON CONFLICT(tenant_id) DO UPDATE \
         SET next_number=work_order_counters.next_number+1 RETURNING next_number",
    )
    .bind(auth.tenant_id())
    .fetch_one(&mut *transaction)
    .await
    .map_err(database_error)?;
    assert_assignment()?;
    let ticket_id = Uuid::new_v4();
    let insert_sql = format!(
        "INSERT INTO public.work_orders(id,number,tenant_id,organization_id,organization_name,\
         owner_user_id,owner_display_name,title,description,priority,assignee_user_id,assignee_display_name) \
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING {RECORD_COLUMNS}"
    );
    let row = sqlx::query(&insert_sql)
        .bind(ticket_id)
        .bind(format!("WO-{number:06}"))
        .bind(auth.tenant_id())
        .bind(auth.organization_id())
        .bind(auth.organization_name())
        .bind(auth.user_id())
        .bind(auth.owner_display_name())
        .bind(&body.title)
        .bind(&body.description)
        .bind(body.priority.as_str())
        .bind(assignee)
        .bind(verified.as_ref().map(|member| member.display_name()))
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
    assert_assignment()?;
    let ticket = Ticket::from_row(row).map_err(database_error)?;
    let response = serde_json::to_string(&ticket).map_err(|_| ApiError::database_unavailable())?;
    if response.len() > MAX_RECEIPT_BYTES {
        return Err(ApiError::database_unavailable());
    }
    let complete_sql = format!(
        "UPDATE public.work_order_create_idempotency SET ticket_id=$5,response_json=$6 \
         WHERE {RECEIPT_DOMAIN} AND request_hash=$7 AND ticket_id IS NULL AND response_json IS NULL"
    );
    let updated = sqlx::query(&complete_sql)
        .bind(auth.tenant_id())
        .bind(auth.organization_id())
        .bind(auth.user_id())
        .bind(idempotency_key)
        .bind(ticket_id)
        .bind(response)
        .bind(request_hash)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    if updated.rows_affected() != 1 {
        return Err(ApiError::database_unavailable());
    }
    assert_assignment()?;
    transaction
        .commit()
        .await
        .map_err(|_| commit_unknown(true))?;
    // Expiry after COMMIT cannot undo the write. Return recovery guidance rather
    // than a credential error that might be interpreted as "nothing changed".
    // A fresh same-domain retry can recover the receipt with the original key.
    assert_assignment().map_err(|_| commit_unknown(true))?;
    Ok(ticket)
}

async fn lock_ticket(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    auth: &Authorized,
    id: Uuid,
) -> Result<PgRow, ApiError> {
    let mut sql = QueryBuilder::new("SELECT ");
    sql.push(RECORD_COLUMNS)
        .push(" FROM public.work_orders WHERE ");
    auth.append_scope(&mut sql);
    let row = sql
        .push(" AND id=")
        .push_bind(id)
        .push(" FOR UPDATE")
        .build()
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
    auth.assert_fresh()?;
    row.ok_or_else(ApiError::not_found)
}

struct AssignmentPreflight {
    organization: Uuid,
    assignee: Option<Uuid>,
}

async fn assignment_preflight(
    pool: &PgPool,
    auth: &Authorized,
    id: Uuid,
    version: i32,
) -> Result<AssignmentPreflight, ApiError> {
    let mut sql = QueryBuilder::new(
        "SELECT organization_id,assignee_user_id,version FROM public.work_orders WHERE ",
    );
    auth.append_scope(&mut sql);
    let row = sql
        .push(" AND id=")
        .push_bind(id)
        .build()
        .fetch_optional(pool)
        .await
        .map_err(database_error)?
        .ok_or_else(ApiError::not_found)?;
    // fetch_optional returns the connection to the pool before the caller
    // starts member HTTP. This read takes no ticket row lock or transaction.
    auth.assert_fresh()?;
    if row.try_get::<i32, _>("version").map_err(database_error)? != version {
        return Err(version_conflict());
    }
    Ok(AssignmentPreflight {
        organization: row.try_get("organization_id").map_err(database_error)?,
        assignee: row.try_get("assignee_user_id").map_err(database_error)?,
    })
}

pub(crate) async fn update(
    pool: &PgPool,
    auth: &Authorized,
    id: &str,
    body: &[u8],
    authorizer: &Authorizer,
    token: &str,
    request_id: &str,
) -> Result<Ticket, ApiError> {
    auth.require_action(Action::Update)?;
    auth.assert_fresh()?;
    let id = dto::canonical_uuid(id)?;
    let body = dto::update(body)?;
    let requested = match &body.assignee_user_id {
        Patch::Absent => None,
        Patch::Present(value) => Some(value.as_deref().map(dto::canonical_uuid).transpose()?),
    };
    let preflight = if requested.is_some() {
        Some(assignment_preflight(pool, auth, id, body.version.0).await?)
    } else {
        None
    };
    let mut replacement = None;
    let mut verified = None;
    if let (Some(target), Some(preflight)) = (requested, &preflight) {
        if target != preflight.assignee {
            auth.require_organization(preflight.organization)?;
            replacement = Some(target);
            if let Some(user) = target {
                verified = Some(
                    authorizer
                        .member(
                            auth,
                            token,
                            preflight.organization,
                            user,
                            Action::Update,
                            request_id,
                        )
                        .await?,
                );
            }
        }
    }
    let mut transaction = crate::telemetry::begin(pool)
        .await
        .map_err(database_error)?;
    auth.assert_fresh()?;
    let row = lock_ticket(&mut transaction, auth, id).await?;
    let version: i32 = row.try_get("version").map_err(database_error)?;
    if version != body.version.0 {
        return Err(version_conflict());
    }
    let organization: Uuid = row.try_get("organization_id").map_err(database_error)?;
    let old_assignee: Option<Uuid> = row.try_get("assignee_user_id").map_err(database_error)?;
    if preflight.as_ref().is_some_and(|preflight| {
        preflight.organization != organization || preflight.assignee != old_assignee
    }) {
        return Err(version_conflict());
    }
    let assert_assignment = || {
        auth.assert_fresh()?;
        if let (Some(member), Some(Some(user))) = (&verified, replacement) {
            member.assert_for(auth, organization, user, Action::Update)?;
        }
        Ok::<(), ApiError>(())
    };
    assert_assignment()?;
    let current_status: String = row.try_get("status").map_err(database_error)?;
    let current_status = dto::Status::from_database(&current_status)?;
    let next_status = match &body.status {
        Patch::Absent => current_status,
        Patch::Present(status) => *status,
    };
    if !current_status.can_transition_to(next_status) {
        return Err(ApiError::validation());
    }
    let title = match &body.title {
        Patch::Absent => row.try_get("title").map_err(database_error)?,
        Patch::Present(value) => value.clone(),
    };
    let description = match &body.description {
        Patch::Absent => row.try_get("description").map_err(database_error)?,
        Patch::Present(value) => value.clone(),
    };
    let priority: String = match &body.priority {
        Patch::Absent => row.try_get("priority").map_err(database_error)?,
        Patch::Present(value) => value.as_str().to_owned(),
    };
    let mut assignee_id = old_assignee;
    let mut assignee_name: Option<String> = row
        .try_get("assignee_display_name")
        .map_err(database_error)?;
    if let Some(target) = replacement {
        auth.require_organization(organization)?;
        assignee_id = target;
        assignee_name = verified
            .as_ref()
            .map(|member| member.display_name().to_owned());
    }
    assert_assignment()?;
    let update_sql = format!(
        "UPDATE public.work_orders SET title=$2,description=$3,priority=$4,status=$5,\
         assignee_user_id=$6,assignee_display_name=$7,version=version+1,\
         updated_at=statement_timestamp() \
         WHERE id=$1 AND tenant_id=$8 AND deleted_at IS NULL AND version=$9 RETURNING {RECORD_COLUMNS}"
    );
    let row = sqlx::query(&update_sql)
        .bind(id)
        .bind(title)
        .bind(description)
        .bind(priority)
        .bind(next_status.as_str())
        .bind(assignee_id)
        .bind(assignee_name)
        .bind(auth.tenant_id())
        .bind(body.version.0)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(version_conflict)?;
    assert_assignment()?;
    let ticket = Ticket::from_row(row).map_err(database_error)?;
    assert_assignment()?;
    transaction
        .commit()
        .await
        .map_err(|_| commit_unknown(false))?;
    assert_assignment().map_err(|_| commit_unknown(false))?;
    Ok(ticket)
}

pub(crate) async fn delete(
    pool: &PgPool,
    auth: &Authorized,
    id: &str,
    body: &[u8],
) -> Result<(), ApiError> {
    auth.require_action(Action::Delete)?;
    auth.assert_fresh()?;
    let id = dto::canonical_uuid(id)?;
    let body = dto::delete(body)?;
    let mut transaction = crate::telemetry::begin(pool)
        .await
        .map_err(database_error)?;
    auth.assert_fresh()?;
    let row = lock_ticket(&mut transaction, auth, id).await?;
    let version: i32 = row.try_get("version").map_err(database_error)?;
    if version != body.version.0 {
        return Err(version_conflict());
    }
    auth.assert_fresh()?;
    let deleted = sqlx::query(
        "UPDATE public.work_orders SET deleted_at=statement_timestamp(),\
         updated_at=statement_timestamp(),version=version+1 \
         WHERE id=$1 AND tenant_id=$2 AND deleted_at IS NULL AND version=$3",
    )
    .bind(id)
    .bind(auth.tenant_id())
    .bind(body.version.0)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    if deleted.rows_affected() != 1 {
        return Err(version_conflict());
    }
    auth.assert_fresh()?;
    transaction
        .commit()
        .await
        .map_err(|_| commit_unknown(false))?;
    auth.assert_fresh().map_err(|_| commit_unknown(false))?;
    Ok(())
}
