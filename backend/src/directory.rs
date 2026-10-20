//! Bounded platform member lookups. This module never opens a business database
//! connection, and a validation witness cannot be constructed by write callers.

mod query;

use std::time::Instant;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    authorization::{Action, Authorized, Authorizer, MemberListQuery},
    error::ApiError,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Member {
    user_id: String,
    display_name: String,
    organization_id: String,
}

#[derive(Serialize)]
pub(crate) struct List {
    records: Vec<Member>,
    total: u64,
    page: u32,
    size: u32,
}

/// An immutable result of a successful, identity-bound platform member lookup.
/// The monotonic deadline begins before its upstream request, never on delivery
/// or after acquiring a ticket lock. Production write callers cannot forge it.
pub(crate) struct VerifiedMember {
    actor: Uuid,
    tenant: Uuid,
    current_organization: Uuid,
    action: Action,
    organization: Uuid,
    user: Uuid,
    display_name: String,
    deadline: Instant,
}

impl VerifiedMember {
    pub(crate) fn assert_for(
        &self,
        auth: &Authorized,
        organization: Uuid,
        user: Uuid,
        action: Action,
    ) -> Result<(), ApiError> {
        auth.require_action(action)?;
        auth.require_organization(organization)?;
        if !self.matches(
            auth.user_id(),
            auth.tenant_id(),
            auth.organization_id(),
            organization,
            user,
            action,
        ) {
            return Err(ApiError::data_denied());
        }
        self.assert_deadline(Instant::now())
    }

    fn matches(
        &self,
        actor: Uuid,
        tenant: Uuid,
        current_organization: Uuid,
        organization: Uuid,
        user: Uuid,
        action: Action,
    ) -> bool {
        self.actor == actor
            && self.tenant == tenant
            && self.current_organization == current_organization
            && self.action == action
            && self.organization == organization
            && self.user == user
    }

    fn assert_deadline(&self, now: Instant) -> Result<(), ApiError> {
        if now >= self.deadline {
            return Err(expired());
        }
        Ok(())
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }
}

fn expired() -> ApiError {
    ApiError::new(
        503,
        "MEMBER_VALIDATION_EXPIRED",
        "处理人校验已到期，请重新提交请求",
    )
}

fn safe_integer(value: &Value) -> Option<u64> {
    let value = value.as_f64()?;
    (value.is_finite() && (0.0..=9_007_199_254_740_991.0).contains(&value) && value.fract() == 0.0)
        .then_some(value as u64)
}

fn member(value: &Value, organization: Uuid) -> Result<(Uuid, String), ApiError> {
    let invalid = ApiError::platform_unavailable;
    let user = value
        .get("id")
        .and_then(Value::as_str)
        .and_then(query::uuid)
        .ok_or_else(invalid)?;
    if value
        .get("organizationId")
        .and_then(Value::as_str)
        .and_then(query::uuid)
        != Some(organization)
    {
        return Err(invalid());
    }
    let display = value
        .get("displayName")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    if query::trim(display).is_empty() || display.chars().count() > 100 || display.contains('\0') {
        return Err(invalid());
    }
    Ok((user, display.to_owned()))
}

fn page(
    value: &Value,
    organization: Uuid,
    requested_page: u32,
    requested_size: u32,
) -> Result<List, ApiError> {
    let invalid = ApiError::platform_unavailable;
    let total = value
        .get("total")
        .and_then(safe_integer)
        .ok_or_else(invalid)?;
    if value.get("page").and_then(safe_integer) != Some(u64::from(requested_page))
        || value.get("size").and_then(safe_integer) != Some(u64::from(requested_size))
    {
        return Err(invalid());
    }
    let records = value
        .get("records")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if records.len() > requested_size as usize {
        return Err(invalid());
    }
    let records = records
        .iter()
        .map(|value| {
            let (user, display_name) = member(value, organization)?;
            Ok(Member {
                user_id: user.to_string(),
                display_name,
                organization_id: organization.to_string(),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(List {
        records,
        total,
        page: requested_page,
        size: requested_size,
    })
}

pub(crate) async fn list(
    authorizer: &Authorizer,
    token: &str,
    request_id: &str,
    raw_query: Option<&str>,
) -> Result<List, ApiError> {
    let query = query::parse(raw_query)?;
    let auth = authorizer
        .authorize(token, query.action, request_id)
        .await?;
    let organization = query.organization.unwrap_or_else(|| auth.organization_id());
    auth.require_organization(organization)?;
    let lookup = MemberListQuery {
        organization,
        action: query.action,
        page: query.page,
        size: query.size,
        q: &query.q,
    };
    let response = authorizer
        .list_members_response(&auth, token, &lookup, request_id)
        .await?;
    auth.assert_fresh()?;
    let result = page(&response, organization, query.page, query.size)?;
    auth.assert_fresh()?;
    Ok(result)
}

impl Authorizer {
    pub(crate) async fn member(
        &self,
        auth: &Authorized,
        token: &str,
        organization: Uuid,
        user: Uuid,
        action: Action,
        request_id: &str,
    ) -> Result<VerifiedMember, ApiError> {
        let (value, deadline) = self
            .member_response(auth, token, organization, user, action, request_id)
            .await?;
        let (returned_user, display_name) = member(&value, organization)?;
        if returned_user != user {
            return Err(ApiError::platform_unavailable());
        }
        let result = VerifiedMember {
            actor: auth.user_id(),
            tenant: auth.tenant_id(),
            current_organization: auth.organization_id(),
            action,
            organization,
            user,
            display_name,
            deadline,
        };
        result.assert_for(auth, organization, user, action)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
