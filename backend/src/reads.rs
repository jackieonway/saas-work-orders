//! Read-only work-order use cases. Every query obtains its tenant, ownership,
//! grant and organization ceiling exclusively from a validated authorization.

mod csv;
mod query;

use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::{
    authorization::{Action, Authorized},
    error::ApiError,
};

use query::Filters;

pub(crate) const RECORD_COLUMNS: &str = "id,number,title,description,priority,status,tenant_id,\
    organization_id,organization_name,owner_user_id,owner_display_name,\
    assignee_user_id,assignee_display_name,version,\
    pg_catalog.to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_iso,\
    pg_catalog.to_char(updated_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_iso";
const ORDER: &str = " ORDER BY created_at DESC,id ASC";
const MAX_EXPORT_ROWS: i64 = 1000;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Ticket {
    id: String,
    number: String,
    title: String,
    description: String,
    priority: String,
    status: String,
    tenant_id: String,
    organization_id: String,
    organization_name: String,
    owner_user_id: String,
    owner_display_name: String,
    assignee_user_id: Option<String>,
    assignee_display_name: Option<String>,
    version: i32,
    created_at: String,
    updated_at: String,
}

impl Ticket {
    pub(crate) fn from_row(row: PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get::<Uuid, _>("id")?.to_string(),
            number: row.try_get("number")?,
            title: row.try_get("title")?,
            description: row.try_get("description")?,
            priority: row.try_get("priority")?,
            status: row.try_get("status")?,
            tenant_id: row.try_get::<Uuid, _>("tenant_id")?.to_string(),
            organization_id: row.try_get::<Uuid, _>("organization_id")?.to_string(),
            organization_name: row.try_get("organization_name")?,
            owner_user_id: row.try_get::<Uuid, _>("owner_user_id")?.to_string(),
            owner_display_name: row.try_get("owner_display_name")?,
            assignee_user_id: row
                .try_get::<Option<Uuid>, _>("assignee_user_id")?
                .map(|value| value.to_string()),
            assignee_display_name: row.try_get("assignee_display_name")?,
            version: row.try_get("version")?,
            created_at: row.try_get("created_iso")?,
            updated_at: row.try_get("updated_iso")?,
        })
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ListResponse {
    records: Vec<Ticket>,
    total: i64,
    page: u32,
    size: u32,
}

fn database_error(_error: sqlx::Error) -> ApiError {
    ApiError::database_unavailable()
}

fn row_limit() -> ApiError {
    ApiError::new(
        400,
        "EXPORT_LIMIT",
        "单次最多导出 1000 条工单，请缩小筛选范围",
    )
}

fn filtered<'a>(
    prefix: &str,
    auth: &'a Authorized,
    filters: &'a Filters,
) -> QueryBuilder<'a, Postgres> {
    let mut sql = QueryBuilder::new(prefix);
    auth.append_scope(&mut sql);
    if let Some(status) = &filters.status {
        sql.push(" AND status=").push_bind(status);
    }
    if let Some(priority) = &filters.priority {
        sql.push(" AND priority=").push_bind(priority);
    }
    if let Some(organization_id) = filters.organization_id {
        sql.push(" AND organization_id=").push_bind(organization_id);
    }
    if let Some(pattern) = &filters.pattern {
        sql.push(" AND (title ILIKE ")
            .push_bind(pattern)
            .push(" ESCAPE CHR(92) OR description ILIKE ")
            .push_bind(pattern)
            .push(" ESCAPE CHR(92) OR number ILIKE ")
            .push_bind(pattern)
            .push(" ESCAPE CHR(92))");
    }
    sql
}

async fn read_snapshot(pool: &PgPool) -> Result<Transaction<'_, Postgres>, ApiError> {
    let mut transaction = crate::telemetry::begin(pool)
        .await
        .map_err(database_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    Ok(transaction)
}

pub(crate) async fn list(
    pool: &PgPool,
    auth: &Authorized,
    raw_query: Option<&str>,
) -> Result<ListResponse, ApiError> {
    let timer = crate::telemetry::registry().timer(operational_telemetry::Stage::BusinessSql);
    let result = list_inner(pool, auth, raw_query).await;
    timer.finish(crate::telemetry::outcome(&result));
    result
}

async fn list_inner(
    pool: &PgPool,
    auth: &Authorized,
    raw_query: Option<&str>,
) -> Result<ListResponse, ApiError> {
    auth.require_action(Action::Read)?;
    auth.assert_fresh()?;
    let query = query::list(raw_query)?;
    let mut transaction = read_snapshot(pool).await?;
    let total: i64 = filtered(
        "SELECT count(*)::bigint FROM public.work_orders WHERE ",
        auth,
        &query.filters,
    )
    .build_query_scalar()
    .fetch_one(&mut *transaction)
    .await
    .map_err(database_error)?;
    auth.assert_fresh()?;
    let rows = filtered(
        &format!("SELECT {RECORD_COLUMNS} FROM public.work_orders WHERE "),
        auth,
        &query.filters,
    )
    .push(ORDER)
    .push(" LIMIT ")
    .push_bind(i64::from(query.size))
    .push(" OFFSET ")
    .push_bind(query.offset)
    .build()
    .fetch_all(&mut *transaction)
    .await
    .map_err(database_error)?;
    let records = rows
        .into_iter()
        .map(Ticket::from_row)
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    auth.assert_fresh()?;
    transaction.commit().await.map_err(database_error)?;
    auth.assert_fresh()?;
    Ok(ListResponse {
        records,
        total,
        page: query.page,
        size: query.size,
    })
}

pub(crate) async fn detail(
    pool: &PgPool,
    auth: &Authorized,
    raw_id: &str,
) -> Result<Ticket, ApiError> {
    let timer = crate::telemetry::registry().timer(operational_telemetry::Stage::BusinessSql);
    let result = detail_inner(pool, auth, raw_id).await;
    timer.finish(crate::telemetry::outcome(&result));
    result
}

async fn detail_inner(pool: &PgPool, auth: &Authorized, raw_id: &str) -> Result<Ticket, ApiError> {
    auth.require_action(Action::Read)?;
    auth.assert_fresh()?;
    let id = query::canonical_uuid(raw_id)?;
    let row = filtered(
        &format!("SELECT {RECORD_COLUMNS} FROM public.work_orders WHERE "),
        auth,
        &Filters::default(),
    )
    .push(" AND id=")
    .push_bind(id)
    .build()
    .fetch_optional(pool)
    .await
    .map_err(database_error)?;
    auth.assert_fresh()?;
    let row = row.ok_or_else(ApiError::not_found)?;
    Ticket::from_row(row).map_err(database_error)
}

pub(crate) async fn export(
    pool: &PgPool,
    auth: &Authorized,
    raw_query: Option<&str>,
) -> Result<Vec<u8>, ApiError> {
    let timer = crate::telemetry::registry().timer(operational_telemetry::Stage::BusinessSql);
    let result = export_inner(pool, auth, raw_query).await;
    timer.finish(crate::telemetry::outcome(&result));
    result
}

async fn export_inner(
    pool: &PgPool,
    auth: &Authorized,
    raw_query: Option<&str>,
) -> Result<Vec<u8>, ApiError> {
    auth.require_action(Action::Export)?;
    auth.assert_fresh()?;
    let filters = query::export(raw_query)?;
    // Capacity and rows use one snapshot: a concurrent writer cannot replace
    // checked small fields with large text before the bounded fetch.
    let mut transaction = read_snapshot(pool).await?;
    let capacity = filtered(
        "WITH selected AS (SELECT number,title,description,organization_name,\
         owner_display_name,assignee_display_name FROM public.work_orders WHERE ",
        auth,
        &filters,
    )
    .push(ORDER)
    .push(" LIMIT 1001) SELECT count(*)::bigint AS total,\
           COALESCE(sum(octet_length(number)::bigint+octet_length(title)+octet_length(description)+\
           octet_length(organization_name)+octet_length(owner_display_name)+\
           octet_length(COALESCE(assignee_display_name,''))),0)::bigint AS minimum_bytes FROM selected")
    .build()
    .fetch_one(&mut *transaction)
    .await
    .map_err(database_error)?;
    auth.assert_fresh()?;
    let total: i64 = capacity.try_get("total").map_err(database_error)?;
    let minimum_bytes: i64 = capacity.try_get("minimum_bytes").map_err(database_error)?;
    if total > MAX_EXPORT_ROWS {
        return Err(row_limit());
    }
    if minimum_bytes > csv::MAX_BYTES as i64 {
        return Err(csv::byte_limit());
    }
    let rows = filtered(
        &format!("SELECT {RECORD_COLUMNS} FROM public.work_orders WHERE "),
        auth,
        &filters,
    )
    .push(ORDER)
    .push(" LIMIT 1001")
    .build()
    .fetch_all(&mut *transaction)
    .await
    .map_err(database_error)?;
    auth.assert_fresh()?;
    if rows.len() > MAX_EXPORT_ROWS as usize {
        return Err(row_limit());
    }
    let records = rows
        .into_iter()
        .map(Ticket::from_row)
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let content = csv::encode(&records)?;
    auth.assert_fresh()?;
    transaction.commit().await.map_err(database_error)?;
    auth.assert_fresh()?;
    Ok(content)
}
