//! Durable reminder event identity followed by one platform publication attempt.
//! Business connections and locks are released before any external POST.

mod dto;
mod store;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use sqlx::PgPool;

use crate::{
    authorization::{Authorizer, ReminderAuthorized},
    error::ApiError,
};

pub(crate) use dto::Publication;

#[derive(Clone, Default)]
pub(crate) struct Attempt(Arc<AtomicBool>);

impl Attempt {
    pub(crate) fn mark(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub(crate) fn attempted(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub(crate) fn preparation_unknown() -> ApiError {
    ApiError::new(
        503,
        "REMINDER_PREPARATION_UNKNOWN",
        "提醒准备结果尚未确认，本次未发送，请保留同一提醒键和原正文重试",
    )
}

pub(crate) fn result_unknown() -> ApiError {
    ApiError::new(
        503,
        "REMINDER_RESULT_UNKNOWN",
        "提醒结果尚未确认，请保留同一提醒键和原正文重试",
    )
}

pub(crate) fn conflict() -> ApiError {
    ApiError::new(
        409,
        "IDEMPOTENCY_CONFLICT",
        "提醒键已用于不同工单或接收人，请明确发起新的独立提醒",
    )
}

pub(crate) async fn send(
    pool: &PgPool,
    auth: &ReminderAuthorized,
    authorizer: &Authorizer,
    token: &str,
    ticket_id: &str,
    body: &[u8],
    metadata: (&str, &Attempt),
) -> Result<Publication, ApiError> {
    auth.assert_fresh()?;
    let id = dto::uuid(ticket_id).ok_or_else(ApiError::validation)?;
    let request = dto::request(body)?;
    let prepared = store::claim(pool, auth, id, &request.idempotency_key).await?;
    // claim consumes and commits its transaction before returning. No connection
    // or mutable work-order guard can be held across the platform request.
    auth.assert_fresh()?;
    authorizer
        .publish_reminder(auth, token, &prepared, metadata.0, metadata.1)
        .await
}
