//! The resource backend trusts the private principal, never public OAuth sub.
use super::*;
use serde::Deserialize;

const INTERNAL_ISSUER: &str = "urn:saas:platform:internal-capabilities";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UserReply {
    contract_version: String,
    issuer: String,
    token_use: String,
    subject_mode: String,
    resource: String,
    action: String,
    target_application_id: String,
    audience: String,
    expires_at: String,
    principal: Principal,
    introspection: Introspection,
    context: Value,
    client_id: Option<String>,
    source_application_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Principal {
    user_id: String,
    membership_id: String,
    member_type: String,
    identity_version: String,
    account_version: String,
    tenant_id: String,
    organization_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Introspection {
    active: bool,
    scope: String,
    client_id: String,
    token_type: String,
    token_use: String,
    iss: String,
    aud: String,
    exp: i64,
    sub: Option<String>,
    jti: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MachineReply {
    contract_version: String,
    issuer: String,
    token_use: String,
    subject_mode: String,
    client_id: String,
    source_application_id: String,
    target_application_id: String,
    resource: String,
    action: String,
    audience: String,
    scope: Vec<String>,
    credential_version: String,
    expires_at: String,
    delegation: Delegation,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Delegation {
    delegation_id: String,
    version: String,
    target_application_id: String,
    tenant_id: String,
    organization_id: Option<String>,
    resource: String,
    action: String,
    scope: String,
    organization_ids: Vec<String>,
    expires_at: String,
}

fn uuid(value: &str) -> Result<Uuid, ApiError> {
    crate::uuid_v7::parse(value).ok_or_else(ApiError::invalid_token)
}

fn generation(value: &str) -> Result<i64, ApiError> {
    value
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0 && v.to_string() == value)
        .ok_or_else(ApiError::invalid_token)
}

fn expiry(value: &str, now: i64) -> Result<i64, ApiError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|v| v.timestamp_millis())
        .filter(|v| *v > now)
        .ok_or_else(ApiError::invalid_token)
}

fn scopes(values: &[String], required: Option<&str>) -> Result<(), ApiError> {
    let expected = required.ok_or_else(ApiError::invalid_token)?;
    if values.is_empty()
        || values.len() > 128
        || values
            .iter()
            .any(|v| v.is_empty() || v.len() > 128 || !v.bytes().all(|c| c.is_ascii_graphic()))
        || values.iter().collect::<BTreeSet<_>>().len() != values.len()
        || !values.iter().any(|value| value == expected)
    {
        return Err(ApiError::invalid_token());
    }
    Ok(())
}

fn user(
    value: &Value,
    action: Action,
    now: i64,
    issuer: Option<&str>,
    required_scope: Option<&str>,
) -> Result<(TokenIdentity, UserReply), ApiError> {
    let reply: UserReply =
        serde_json::from_value(value.clone()).map_err(|_| ApiError::platform_unavailable())?;
    let intro = &reply.introspection;
    let end = expiry(&reply.expires_at, now)?;
    let token_end = i128::from(intro.exp) * 1000;
    if reply.contract_version != "resource-authorization/1"
        || reply.subject_mode != "USER"
        || reply.resource != "work-orders"
        || reply.action != action.as_str()
        || reply.audience != "work-orders"
        || intro.aud != reply.audience
        || !intro.active
        || intro.token_type != "Bearer"
        || intro.token_use != reply.token_use
        || intro.iss != reply.issuer
        || token_end <= i128::from(now)
        || i128::from(end) > token_end
        || reply.principal.member_type != "EMPLOYEE"
        || reply
            .context
            .pointer("/application/id")
            .and_then(Value::as_str)
            != Some(reply.target_application_id.as_str())
    {
        return Err(ApiError::invalid_token());
    }
    uuid(&reply.target_application_id)?;
    uuid(&reply.principal.membership_id)?;
    generation(&reply.principal.identity_version)?;
    generation(&reply.principal.account_version)?;
    match reply.token_use.as_str() {
        "app_session" => {
            if reply.issuer != INTERNAL_ISSUER
                || intro.scope != "app:access"
                || intro.client_id != "work-orders"
                || intro.sub.is_some()
                || reply.client_id.is_some()
                || reply.source_application_id.is_some()
                || value.as_object().is_some_and(|v| {
                    v.contains_key("clientId") || v.contains_key("sourceApplicationId")
                })
                || value["introspection"]
                    .as_object()
                    .is_some_and(|v| v.contains_key("sub"))
            {
                return Err(ApiError::invalid_token());
            }
            uuid(intro.jti.as_deref().ok_or_else(ApiError::invalid_token)?)?;
        }
        "access_token" => {
            if issuer != Some(reply.issuer.as_str())
                || intro.sub.as_ref().is_none_or(|v| {
                    v.is_empty() || v.len() > 255 || !v.bytes().all(|c| c.is_ascii_graphic())
                })
                || intro.jti.is_some()
                || value["introspection"]
                    .as_object()
                    .is_some_and(|v| v.contains_key("jti"))
            {
                return Err(ApiError::invalid_token());
            }
            let client = reply
                .client_id
                .as_deref()
                .ok_or_else(ApiError::invalid_token)?;
            uuid(client)?;
            uuid(
                reply
                    .source_application_id
                    .as_deref()
                    .ok_or_else(ApiError::invalid_token)?,
            )?;
            if intro.client_id != client {
                return Err(ApiError::invalid_token());
            }
            scopes(
                &intro
                    .scope
                    .split(' ')
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
                required_scope,
            )?;
        }
        _ => return Err(ApiError::invalid_token()),
    }
    let identity = TokenIdentity {
        user: uuid(&reply.principal.user_id)?,
        tenant: uuid(&reply.principal.tenant_id)?,
        organization: uuid(&reply.principal.organization_id)?,
        expires_at: i128::from(end).min(token_end),
    };
    Ok((identity, reply))
}

#[cfg(test)]
mod tests;

pub(in crate::authorization) fn validate_authorization(
    value: &Value,
    action: Action,
    now: i64,
    issuer: Option<&str>,
    required_scope: Option<&str>,
) -> Result<Authorized, ApiError> {
    let mut authorized = match value["subjectMode"].as_str() {
        Some("USER") => {
            let (identity, reply) = user(value, action, now, issuer, required_scope)?;
            validate_context(&identity, &reply.context, action, now)
        }
        Some("MACHINE") => machine(value, action, now, issuer, required_scope),
        _ => Err(ApiError::invalid_token()),
    }?;
    // Compare original authority/projection facts across online checkpoints.
    // Only informational response deadlines may shorten independently.
    let mut stable = value.clone();
    if let Some(map) = stable.as_object_mut() {
        map.remove("expiresAt");
    }
    if let Some(map) = stable.get_mut("context").and_then(Value::as_object_mut) {
        map.remove("expiresAt");
    }
    if let Some(map) = stable.get_mut("delegation").and_then(Value::as_object_mut) {
        map.remove("expiresAt");
    }
    use sha2::Digest;
    authorized.source_binding =
        format!("{:x}", sha2::Sha256::digest(stable.to_string().as_bytes()));
    Ok(authorized)
}

fn machine(
    value: &Value,
    action: Action,
    now: i64,
    issuer: Option<&str>,
    required_scope: Option<&str>,
) -> Result<Authorized, ApiError> {
    if !matches!(action, Action::Read | Action::Export) {
        return Err(operation_denied());
    }
    let reply: MachineReply =
        serde_json::from_value(value.clone()).map_err(|_| ApiError::platform_unavailable())?;
    let d = &reply.delegation;
    if reply.contract_version != "resource-authorization/1"
        || reply.subject_mode != "MACHINE"
        || reply.token_use != "access_token"
        || issuer != Some(reply.issuer.as_str())
        || reply.resource != "work-orders"
        || reply.audience != "work-orders"
        || reply.action != action.as_str()
        || d.action != reply.action
        || d.resource != "ticket"
        || d.target_application_id != reply.target_application_id
    {
        return Err(ApiError::invalid_token());
    }
    let client_id = uuid(&reply.client_id)?;
    let source_application_id = uuid(&reply.source_application_id)?;
    uuid(&reply.target_application_id)?;
    let tenant_id = uuid(&d.tenant_id)?;
    let organization = d.organization_id.as_deref().map(uuid).transpose()?;
    let ids = d
        .organization_ids
        .iter()
        .map(|v| uuid(v))
        .collect::<Result<Vec<_>, _>>()?;
    if ids.len() > 1000 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err(ApiError::data_denied());
    }
    let scope = match d.scope.as_str() {
        "ORG" if organization.is_some_and(|org| ids == [org]) => Scope::Org,
        "ORG_SUBTREE" if !ids.is_empty() && organization.is_some_and(|org| ids.contains(&org)) => {
            Scope::OrgSubtree
        }
        "TENANT" if organization.is_none() && ids.is_empty() => Scope::Tenant,
        _ => return Err(ApiError::data_denied()),
    };
    scopes(&reply.scope, required_scope)?;
    let expires_at = expiry(&reply.expires_at, now)?;
    if expires_at > expiry(&d.expires_at, now)? {
        return Err(ApiError::invalid_token());
    }
    Ok(Authorized {
        tenant_id,
        actor: Actor::Machine {
            client_id,
            source_application_id,
            credential_version: generation(&reply.credential_version)?,
            delegation_id: uuid(&d.delegation_id)?,
            delegation_version: generation(&d.version)?,
            organization_id: organization,
        },
        organization_ceiling: (scope != Scope::Tenant).then(|| ids.clone()),
        scope,
        grant_organization_ids: ids,
        action,
        expires_at,
        source_binding: String::new(),
    })
}

pub(in crate::authorization) fn validate_reminder_authorization(
    value: &Value,
    now: i64,
    issuer: Option<&str>,
    required_scope: Option<&str>,
    notification_scope: Option<&str>,
) -> Result<ReminderAuthorized, ApiError> {
    if value["subjectMode"] != "USER" {
        return Err(operation_denied());
    }
    let read = validate_authorization(value, Action::Read, now, issuer, required_scope)?;
    let (identity, reply) = user(value, Action::Read, now, issuer, required_scope)?;
    if reply.token_use == "access_token" {
        // The read proof carries the original resource token's complete scope
        // set. Require the separately reviewed publish scope before preparing
        // any local intent; the private publisher verifies it again itself.
        scopes(
            &reply
                .introspection
                .scope
                .split(' ')
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            notification_scope,
        )?;
    }
    let mut authorized = validate_reminder(&identity, &reply.context, now)?;
    authorized.read.source_binding = read.source_binding;
    Ok(authorized)
}
