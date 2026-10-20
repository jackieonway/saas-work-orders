//! A per-request authorization boundary. No successful authorization is cached.

mod client;
mod policy;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use sqlx::{Postgres, QueryBuilder};
use tokio::sync::Semaphore;

use crate::{
    error::{ApiError, ServiceError, ServiceFailure},
    Config,
};

pub(crate) use policy::{Action, Authorized, ReminderAuthorized, Scope};

pub(crate) struct MemberListQuery<'a> {
    pub(crate) organization: uuid::Uuid,
    pub(crate) action: Action,
    pub(crate) page: u32,
    pub(crate) size: u32,
    pub(crate) q: &'a str,
}

#[derive(Clone)]
pub(crate) struct Authorizer {
    client: client::PlatformClient,
    concurrent: Arc<Semaphore>,
    timeout: std::time::Duration,
}

impl Authorizer {
    pub(crate) fn new(config: &Config) -> Result<Self, ServiceError> {
        let settings = config.platform.as_ref().ok_or_else(|| {
            ServiceError::caused_by(
                ServiceFailure::PlatformConfiguration,
                std::io::Error::other("required platform settings absent"),
            )
        })?;
        Ok(Self {
            client: client::PlatformClient::new(settings)?,
            concurrent: Arc::new(Semaphore::new(settings.max_concurrent)),
            timeout: settings.timeout,
        })
    }

    /// The permit covers both HTTP calls and their bounded bodies. Saturated
    /// callers fail immediately, so incoming requests cannot form an unbounded
    /// queue of authorization futures or hold a database transaction while waiting.
    pub(crate) async fn authorize(
        &self,
        token: &str,
        action: Action,
        request_id: &str,
    ) -> Result<Authorized, ApiError> {
        let timer = crate::telemetry::registry().timer(operational_telemetry::Stage::Authorization);
        let result = self.authorize_inner(token, action, request_id).await;
        timer.finish(crate::telemetry::outcome(&result));
        result
    }

    async fn authorize_inner(
        &self,
        token: &str,
        action: Action,
        request_id: &str,
    ) -> Result<Authorized, ApiError> {
        if token.is_empty() || token.len() > 8185 || token.chars().any(char::is_whitespace) {
            return Err(ApiError::invalid_token());
        }
        let _permit = self
            .concurrent
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::platform_unavailable())?;
        tokio::time::timeout(self.timeout, async {
            let (intro, context) = self
                .client
                .authorization_pair(token, "ticket", action.as_str(), request_id)
                .await?;
            let identity = policy::validate_intro(&intro, policy::now_millis())?;
            policy::validate_context(&identity, &context, action, policy::now_millis())
        })
        .await
        .map_err(|_| ApiError::platform_unavailable())?
    }

    pub(crate) async fn authorize_reminder(
        &self,
        token: &str,
        request_id: &str,
    ) -> Result<ReminderAuthorized, ApiError> {
        if token.is_empty() || token.len() > 8185 || token.chars().any(char::is_whitespace) {
            return Err(ApiError::invalid_token());
        }
        let _permit = self
            .concurrent
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::platform_unavailable())?;
        tokio::time::timeout(self.timeout, async {
            let (intro, context) = self
                .client
                .authorization_pair(token, "ticket", "read", request_id)
                .await?;
            let identity = policy::validate_intro(&intro, policy::now_millis())?;
            policy::validate_reminder(&identity, &context, policy::now_millis())
        })
        .await
        .map_err(|_| ApiError::platform_unavailable())?
    }

    pub(crate) async fn publish_reminder(
        &self,
        auth: &ReminderAuthorized,
        token: &str,
        json: &str,
        request_id: &str,
        attempt: &crate::reminders::Attempt,
    ) -> Result<crate::reminders::Publication, ApiError> {
        auth.assert_fresh()?;
        let _permit = self
            .concurrent
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::platform_unavailable())?;
        auth.assert_fresh()?;
        let result = tokio::time::timeout(
            self.timeout,
            self.client
                .publish_reminder(token, json, request_id, attempt),
        )
        .await
        .map_err(|_| {
            if attempt.attempted() {
                crate::reminders::result_unknown()
            } else {
                crate::reminders::preparation_unknown()
            }
        })??;
        auth.assert_fresh()
            .map_err(|_| crate::reminders::result_unknown())?;
        Ok(result)
    }

    pub(crate) async fn member_response(
        &self,
        auth: &Authorized,
        token: &str,
        organization: uuid::Uuid,
        user: uuid::Uuid,
        action: Action,
        request_id: &str,
    ) -> Result<(serde_json::Value, Instant), ApiError> {
        directory_authorized(auth, organization, action)?;
        let budget = self.timeout.min(Duration::from_secs(3));
        let deadline = Instant::now() + budget;
        let _permit = self
            .concurrent
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::platform_unavailable())?;
        let value = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            self.client
                .member(token, organization, user, action, request_id),
        )
        .await
        .map_err(|_| ApiError::platform_unavailable())??;
        auth.assert_fresh()?;
        Ok((value, deadline))
    }

    pub(crate) async fn list_members_response(
        &self,
        auth: &Authorized,
        token: &str,
        query: &MemberListQuery<'_>,
        request_id: &str,
    ) -> Result<serde_json::Value, ApiError> {
        directory_authorized(auth, query.organization, query.action)?;
        let _permit = self
            .concurrent
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::platform_unavailable())?;
        let value =
            tokio::time::timeout(self.timeout, self.client.members(token, query, request_id))
                .await
                .map_err(|_| ApiError::platform_unavailable())??;
        auth.assert_fresh()?;
        Ok(value)
    }
}

fn directory_authorized(
    auth: &Authorized,
    organization: uuid::Uuid,
    action: Action,
) -> Result<(), ApiError> {
    if !matches!(action, Action::Create | Action::Update) {
        return Err(ApiError::data_denied());
    }
    auth.require_action(action)?;
    auth.require_organization(organization)
}

impl Authorized {
    /// Append the complete parameterized predicate to a query that has already
    /// emitted WHERE. Every read, count and export uses this same boundary.
    pub(crate) fn append_scope(&self, query: &mut QueryBuilder<'_, Postgres>) {
        query
            .push("tenant_id=")
            .push_bind(self.tenant_id())
            .push(" AND deleted_at IS NULL");
        if let Some(ceiling) = self.organization_ceiling() {
            query
                .push(" AND organization_id=ANY(")
                .push_bind(ceiling.to_vec())
                .push("::uuid[])");
        }
        match self.scope() {
            Scope::SelfOnly => {
                query.push(" AND owner_user_id=").push_bind(self.user_id());
                if !self.grant_organization_ids().is_empty() {
                    query
                        .push(" AND organization_id=ANY(")
                        .push_bind(self.grant_organization_ids().to_vec())
                        .push("::uuid[])");
                }
            }
            Scope::Org => {
                query
                    .push(" AND organization_id=")
                    .push_bind(self.organization_id());
            }
            Scope::OrgSubtree => {
                query
                    .push(" AND organization_id=ANY(")
                    .push_bind(self.grant_organization_ids().to_vec())
                    .push("::uuid[])");
            }
            Scope::Tenant => {}
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod reminder_tests;
