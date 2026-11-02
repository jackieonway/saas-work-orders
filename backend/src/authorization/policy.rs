use std::collections::BTreeSet;

use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

mod resource;
pub(super) use resource::{validate_authorization, validate_reminder_authorization};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Read,
    Export,
    Create,
    Update,
    Delete,
}

impl Action {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Export => "export",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    SelfOnly,
    Org,
    OrgSubtree,
    Tenant,
}

/// This value is constructed only after fresh introspection and context checks.
/// It deliberately contains no token or confidential client credential.
#[derive(Clone, PartialEq, Eq)]
enum Actor {
    User {
        user_id: Uuid,
        organization_id: Uuid,
        organization_name: String,
        owner_display_name: String,
    },
    Machine {
        client_id: Uuid,
        source_application_id: Uuid,
        credential_version: i64,
        delegation_id: Uuid,
        delegation_version: i64,
        organization_id: Option<Uuid>,
    },
}

#[derive(Clone)]
pub(crate) struct Authorized {
    tenant_id: Uuid,
    actor: Actor,
    organization_ceiling: Option<Vec<Uuid>>,
    scope: Scope,
    grant_organization_ids: Vec<Uuid>,
    action: Action,
    expires_at: i64,
    source_binding: String,
}

pub(crate) struct ReminderAuthorized {
    read: Authorized,
    application: Uuid,
    notification_scope: Scope,
    notification_organizations: Vec<Uuid>,
}

impl ReminderAuthorized {
    pub(crate) fn source_binding(&self) -> &str {
        &self.read.source_binding
    }
    pub(crate) fn read(&self) -> &Authorized {
        &self.read
    }
    pub(crate) fn application_id(&self) -> Uuid {
        self.application
    }
    pub(crate) fn assert_fresh(&self) -> Result<(), ApiError> {
        self.read.assert_fresh()
    }
    pub(crate) fn assert_recipient(&self, organization: Uuid, user: Uuid) -> Result<(), ApiError> {
        self.assert_fresh()?;
        if self
            .read
            .organization_ceiling
            .as_ref()
            .is_some_and(|ids| !ids.contains(&organization))
        {
            return Err(ApiError::data_denied());
        }
        let allowed = match self.notification_scope {
            Scope::SelfOnly => {
                organization == self.read.organization_id()? && user == self.read.user_id()?
            }
            Scope::Org => organization == self.read.organization_id()?,
            Scope::OrgSubtree => self.notification_organizations.contains(&organization),
            Scope::Tenant => true,
        };
        if allowed {
            Ok(())
        } else {
            Err(ApiError::data_denied())
        }
    }
}

impl Authorized {
    pub(super) fn same_authority(&self, other: &Self) -> bool {
        self.source_binding == other.source_binding
            && self.actor == other.actor
            && self.tenant_id == other.tenant_id
            && self.organization_ceiling == other.organization_ceiling
            && self.scope == other.scope
            && self.grant_organization_ids == other.grant_organization_ids
            && self.action == other.action
    }
    pub(super) fn retain_deadline(&mut self, original: &Self) {
        self.expires_at = self.expires_at.min(original.expires_at);
    }
    pub(crate) fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }
    pub(crate) fn require_person(&self) -> Result<(), ApiError> {
        if matches!(self.actor, Actor::User { .. }) {
            Ok(())
        } else {
            Err(ApiError::data_denied())
        }
    }
    pub(crate) fn user_id(&self) -> Result<Uuid, ApiError> {
        match self.actor {
            Actor::User { user_id, .. } => Ok(user_id),
            Actor::Machine { .. } => Err(ApiError::data_denied()),
        }
    }
    pub(crate) fn organization_id(&self) -> Result<Uuid, ApiError> {
        match self.actor {
            Actor::User {
                organization_id, ..
            } => Ok(organization_id),
            Actor::Machine { .. } => Err(ApiError::data_denied()),
        }
    }
    pub(crate) fn organization_name(&self) -> Result<&str, ApiError> {
        match &self.actor {
            Actor::User {
                organization_name, ..
            } => Ok(organization_name),
            Actor::Machine { .. } => Err(ApiError::data_denied()),
        }
    }
    pub(crate) fn owner_display_name(&self) -> Result<&str, ApiError> {
        match &self.actor {
            Actor::User {
                owner_display_name, ..
            } => Ok(owner_display_name),
            Actor::Machine { .. } => Err(ApiError::data_denied()),
        }
    }
    pub(super) fn filter_user(&self) -> Option<Uuid> {
        match self.actor {
            Actor::User { user_id, .. } => Some(user_id),
            Actor::Machine { .. } => None,
        }
    }
    pub(super) fn filter_organization(&self) -> Option<Uuid> {
        match self.actor {
            Actor::User {
                organization_id, ..
            } => Some(organization_id),
            Actor::Machine {
                organization_id, ..
            } => organization_id,
        }
    }
    /// Creation and assignment changes must stay in the directory-like target
    /// boundary. An existing SELF ticket may still be updated in a visible child
    /// organization without changing its assignment.
    pub(crate) fn require_organization(&self, target: Uuid) -> Result<(), ApiError> {
        self.require_person()?;
        self.assert_fresh()?;
        if self
            .organization_ceiling
            .as_ref()
            .is_some_and(|ids| !ids.contains(&target))
        {
            return Err(ApiError::data_denied());
        }
        let allowed = match self.scope {
            Scope::SelfOnly | Scope::Org => target == self.organization_id()?,
            Scope::OrgSubtree => self.grant_organization_ids.contains(&target),
            Scope::Tenant => true,
        };
        if allowed {
            Ok(())
        } else {
            Err(ApiError::data_denied())
        }
    }
    pub(crate) fn organization_ceiling(&self) -> Option<&[Uuid]> {
        self.organization_ceiling.as_deref()
    }
    pub(crate) fn scope(&self) -> Scope {
        self.scope
    }
    pub(super) fn action(&self) -> Action {
        self.action
    }
    pub(crate) fn grant_organization_ids(&self) -> &[Uuid] {
        &self.grant_organization_ids
    }
    pub(crate) fn assert_fresh(&self) -> Result<(), ApiError> {
        if self.expires_at <= now_millis() {
            Err(ApiError::invalid_context())
        } else {
            Ok(())
        }
    }
    pub(crate) fn require_action(&self, action: Action) -> Result<(), ApiError> {
        if self.action == action {
            Ok(())
        } else {
            Err(operation_denied())
        }
    }
}

pub(super) struct TokenIdentity {
    user: Uuid,
    tenant: Uuid,
    organization: Uuid,
    expires_at: i128,
}

pub(super) fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn id(value: &Value) -> Option<Uuid> {
    crate::uuid_v7::parse(value.as_str()?)
}

pub(super) fn validate_context(
    identity: &TokenIdentity,
    context: &Value,
    action: Action,
    now: i64,
) -> Result<Authorized, ApiError> {
    let context_error = ApiError::invalid_context;
    let expires_at = context
        .get("expiresAt")
        .and_then(Value::as_str)
        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .map(|time| time.timestamp_millis())
        .ok_or_else(context_error)?;
    let hash = context
        .get("contextVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    let organization_name = context
        .pointer("/organization/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let display_name_valid = match context.pointer("/user/displayName") {
        None | Some(Value::Null) => true,
        Some(Value::String(name)) => name.chars().count() <= 100,
        _ => false,
    };
    if context.get("contractVersion").and_then(Value::as_str) != Some("1.0")
        || context.get("appCode").and_then(Value::as_str) != Some("work-orders")
        || hash.len() != 64
        || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        || expires_at <= now
        || expires_at > now.saturating_add(30_000)
        || i128::from(expires_at) > identity.expires_at + 999
        || context.pointer("/user/id").and_then(id) != Some(identity.user)
        || context.pointer("/tenant/id").and_then(id) != Some(identity.tenant)
        || context.pointer("/organization/id").and_then(id) != Some(identity.organization)
        || context.pointer("/application/id").and_then(id).is_none()
        || context
            .pointer("/user/username")
            .and_then(Value::as_str)
            .is_none()
        || organization_name.trim().is_empty()
        || organization_name.chars().count() > 100
        || !display_name_valid
    {
        return Err(context_error());
    }
    let organization_ceiling = match context.get("organizationScopeIds") {
        None => Some(vec![identity.organization]),
        Some(Value::Null) => None,
        Some(value) => {
            let ids = organization_ids(value).ok_or_else(context_error)?;
            if !ids.contains(&identity.organization) {
                return Err(context_error());
            }
            Some(ids)
        }
    };
    let (scope, organization_ids) = grant(context, identity, "ticket", action.as_str())?;
    // Context expiration may use fractional seconds up to the token's second;
    // requests still stop at the stricter token instant, matching the old API.
    let effective_expiry = i128::from(expires_at).min(identity.expires_at) as i64;
    if effective_expiry <= now {
        return Err(context_error());
    }
    let owner_display_name = context
        .pointer("/user/displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| context.pointer("/user/username").and_then(Value::as_str))
        .ok_or_else(context_error)?;
    if action == Action::Create
        && (owner_display_name.trim().is_empty()
            || owner_display_name.chars().count() > 100
            || owner_display_name.contains('\0')
            || organization_name.contains('\0'))
    {
        return Err(context_error());
    }
    Ok(Authorized {
        tenant_id: identity.tenant,
        actor: Actor::User {
            user_id: identity.user,
            organization_id: identity.organization,
            organization_name: organization_name.to_owned(),
            owner_display_name: owner_display_name.to_owned(),
        },
        organization_ceiling,
        scope,
        grant_organization_ids: organization_ids,
        action,
        expires_at: effective_expiry,
        source_binding: hash.to_owned(),
    })
}

pub(super) fn validate_reminder(
    identity: &TokenIdentity,
    context: &Value,
    now: i64,
) -> Result<ReminderAuthorized, ApiError> {
    let read = validate_context(identity, context, Action::Read, now)?;
    let (notification_scope, notification_organizations) =
        grant(context, identity, "notification", "publish")?;
    let application = context
        .pointer("/application/id")
        .and_then(id)
        .ok_or_else(ApiError::invalid_context)?;
    Ok(ReminderAuthorized {
        read,
        application,
        notification_scope,
        notification_organizations,
    })
}

fn grant(
    context: &Value,
    identity: &TokenIdentity,
    resource: &str,
    action: &str,
) -> Result<(Scope, Vec<Uuid>), ApiError> {
    let permission = format!("{resource}:{action}");
    if !context
        .get("permissions")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(&permission)))
    {
        return Err(operation_denied());
    }
    let grants = context
        .get("dataGrants")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut matching = grants.iter().filter(|grant| {
        grant.get("resource").and_then(Value::as_str) == Some(resource)
            && grant.get("action").and_then(Value::as_str) == Some(action)
    });
    let grant = matching.next().ok_or_else(ApiError::data_denied)?;
    if matching.next().is_some()
        || grant.get("tenantId").and_then(id) != Some(identity.tenant)
        || grant.get("subjectUserId").and_then(id) != Some(identity.user)
    {
        return Err(ApiError::data_denied());
    }
    let organization_ids = grant
        .get("organizationIds")
        .and_then(organization_ids)
        .ok_or_else(ApiError::data_denied)?;
    let scope = match grant.get("scope").and_then(Value::as_str) {
        Some("SELF")
            if organization_ids.is_empty() || organization_ids.contains(&identity.organization) =>
        {
            Scope::SelfOnly
        }
        Some("ORG") if organization_ids == [identity.organization] => Scope::Org,
        Some("ORG_SUBTREE") if organization_ids.contains(&identity.organization) => {
            Scope::OrgSubtree
        }
        Some("TENANT") if organization_ids.is_empty() => Scope::Tenant,
        _ => return Err(ApiError::data_denied()),
    };
    Ok((scope, organization_ids))
}

fn organization_ids(value: &Value) -> Option<Vec<Uuid>> {
    let values = value.as_array()?;
    if values.len() > 10_000 {
        return None;
    }
    values
        .iter()
        .map(id)
        .collect::<Option<BTreeSet<_>>>()
        .map(|ids| ids.into_iter().collect())
}

pub(super) fn operation_denied() -> ApiError {
    ApiError::new(403, "OPERATION_DENIED", "当前用户没有该操作权限")
}
