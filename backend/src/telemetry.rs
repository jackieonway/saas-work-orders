//! Private transport/SQL adapter for the closed operational telemetry protocol.
use crate::error::ApiError;
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use operational_telemetry::{Outcome, Registry, Service, Stage};
use sqlx::{PgPool, Postgres, Transaction};
use std::{sync::OnceLock, time::Duration};
pub(crate) fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry::new(Service::WorkOrders))
}
pub(crate) fn outcome<T>(result: &Result<T, ApiError>) -> Outcome {
    result
        .as_ref()
        .map(|_| Outcome::Ok)
        .unwrap_or_else(|e| Outcome::http(e.status()))
}
pub(crate) async fn begin(pool: &PgPool) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let timer = registry().timer(Stage::PoolAcquire);
    let result = pool.begin().await;
    timer.finish(if result.is_ok() {
        Outcome::Ok
    } else {
        Outcome::Dependency
    });
    result
}
pub(crate) async fn metrics(
    pool: &PgPool,
    expected: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let provided = values
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let valid = values.next().is_none()
        && expected.zip(provided).is_some_and(|(a, b)| {
            !a.is_empty()
                && a.len() == b.len()
                && a.bytes().zip(b.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
        });
    if !valid {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let locks=tokio::time::timeout(Duration::from_millis(500),sqlx::query_scalar::<_,i64>("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'").fetch_one(pool)).await;
    let mut text = registry().render();
    text.push_str(&format!("# TYPE saas_db_pool_size gauge\nsaas_db_pool_size{{service=\"work-orders\"}} {}\n# TYPE saas_db_pool_idle gauge\nsaas_db_pool_idle{{service=\"work-orders\"}} {}\n# TYPE saas_db_lock_waiters gauge\nsaas_db_lock_waiters{{service=\"work-orders\"}} {}\n# TYPE saas_database_up gauge\nsaas_database_up{{service=\"work-orders\"}} {}\n",pool.size(),pool.num_idle(),locks.as_ref().ok().and_then(|v|v.as_ref().ok()).copied().unwrap_or(0),u8::from(locks.is_ok_and(|v|v.is_ok()))));
    (
        [
            (header::CONTENT_TYPE, "text/plain; version=0.0.4"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        text,
    )
        .into_response()
}
