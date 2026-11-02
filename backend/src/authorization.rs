//! A per-request authorization boundary. No successful authorization is cached.

mod client;
mod policy;

#[cfg(test)]
mod node_fixture;

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
    /// Recheck the same original token/operation without a business connection.
    /// A changed principal, source generation or grant cannot replace the
    /// original authorization while a local write is being prepared.
    pub(crate) async fn recheck(
        &self,
        original: &Authorized,
        token: &str,
        request_id: &str,
    ) -> Result<Authorized, ApiError> {
        original.assert_fresh()?;
        let mut current = self.authorize(token, original.action(), request_id).await?;
        if !original.same_authority(&current) {
            return Err(ApiError::invalid_context());
        }
        current.retain_deadline(original);
        current.assert_fresh()?;
        Ok(current)
    }
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

    /// The permit covers the private authorization call and its bounded body. Saturated
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
            let reply = self
                .client
                .authorization(token, "ticket", action.as_str(), request_id)
                .await?;
            policy::validate_authorization(
                &reply,
                action,
                policy::now_millis(),
                self.client.identity_issuer(),
                self.client.required_scope(action),
            )
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
            let reply = self
                .client
                .authorization(token, "ticket", "read", request_id)
                .await?;
            policy::validate_reminder_authorization(
                &reply,
                policy::now_millis(),
                self.client.identity_issuer(),
                self.client.required_scope(Action::Read),
                self.client.notification_scope(),
            )
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
            self.timeout.min(Duration::from_secs(3)),
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
        let (value, server_deadline) = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            self.client
                .member(token, organization, user, action, request_id),
        )
        .await
        .map_err(|_| ApiError::platform_unavailable())??;
        auth.assert_fresh()?;
        Ok((value, deadline.min(server_deadline)))
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
        let (value, server_deadline) =
            tokio::time::timeout(self.timeout, self.client.members(token, query, request_id))
                .await
                .map_err(|_| ApiError::platform_unavailable())??;
        auth.assert_fresh()?;
        if Instant::now() >= server_deadline {
            return Err(ApiError::platform_unavailable());
        }
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
                query
                    .push(" AND owner_user_id=")
                    .push_bind(self.filter_user());
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
                    .push_bind(self.filter_organization());
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
