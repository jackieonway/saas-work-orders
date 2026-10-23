//! Generated public wire DTOs. Regenerate with the SDK fixed entry; do not edit.
//! No platform/database dependency, authentication, token refresh, or shared storage access.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
/// The only registered SDK release; wire contract majors remain independently versioned.
pub const SDK_VERSION: &str = "2.0.0";
/// Context wire contract supported by this release.
pub const CONTEXT_VERSION: &str = "1.0";
/// Props wire contract supported by this release.
pub const MICRO_PROPS_VERSION: &str = "2.0";
/// Notification wire contract supported by this release.
pub const NOTIFICATION_VERSION: &str = "1.0";
/// Maximum bounded public JSON response bytes.
pub const RESPONSE_LIMIT_BYTES: usize = 1048576;
/// Maximum explicitly listed organizations; never silently truncated.
pub const ORGANIZATION_LIMIT: usize = 10000;

/// Public wire type generated from `$defs.SDKVersion`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SDKVersion {
    #[serde(rename = "2.0.0")]
    V2Dot0Dot0,
}

/// Public wire type generated from `$defs.ContextContractVersion`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextContractVersion {
    #[serde(rename = "1.0")]
    V1Dot0,
}

/// Public wire type generated from `$defs.MicroPropsContractVersion`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicroPropsContractVersion {
    #[serde(rename = "2.0")]
    V2Dot0,
}

/// Public wire type generated from `$defs.NotificationContractVersion`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationContractVersion {
    #[serde(rename = "1.0")]
    V1Dot0,
}

/// Public wire type generated from `$defs.MicroEntryType`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicroEntryType {
    #[serde(rename = "HTML")]
    Html,
}

/// Public wire type generated from `$defs.MicroRuntimeMode`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicroRuntimeMode {
    #[serde(rename = "TRUSTED")]
    Trusted,
}

/// Public wire type generated from `$defs.MicroTheme`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicroTheme {
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}

/// Public wire type generated from `$defs.MicroNotifyVariant`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicroNotifyVariant {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "info")]
    Info,
}

/// Public wire type generated from `$defs.MicroDataScope`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicroDataScope {
    #[serde(rename = "SELF")]
    SelfScope,
    #[serde(rename = "ORG")]
    Org,
    #[serde(rename = "ORG_SUBTREE")]
    OrgSubtree,
    #[serde(rename = "TENANT")]
    Tenant,
}

/// Public wire type generated from `$defs.NotificationType`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationType {
    #[serde(rename = "INFO")]
    Info,
    #[serde(rename = "SUCCESS")]
    Success,
    #[serde(rename = "WARNING")]
    Warning,
    #[serde(rename = "SECURITY")]
    Security,
}

/// Public wire type generated from `$defs.MicroUser`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroUser {
    pub id: String,
    pub username: String,
    pub display_name: String,
}

/// Public wire type generated from `$defs.MicroTenant`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroTenant {
    pub id: String,
    pub code: String,
    pub name: String,
}

/// Public wire type generated from `$defs.MicroOrganization`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroOrganization {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// Public wire type generated from `$defs.MicroRole`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroRole {
    pub id: String,
    pub name: String,
    pub code: String,
}

/// Public wire type generated from `$defs.MicroDataGrant`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroDataGrant {
    pub resource: String,
    pub action: String,
    pub scope: MicroDataScope,
    pub subject_user_id: String,
    pub tenant_id: String,
    pub organization_ids: Vec<String>,
}

/// Public wire type generated from `$defs.MicroMenuType`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicroMenuType {
    #[serde(rename = "DIR")]
    Dir,
    #[serde(rename = "MENU")]
    Menu,
    #[serde(rename = "BUTTON")]
    Button,
}

/// Public wire type generated from `$defs.MicroMenuItem`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroMenuItem {
    pub id: String,
    #[serde(deserialize_with = "required_nullable")]
    pub parent_id: Option<String>,
    pub r#type: MicroMenuType,
    pub title: String,
    #[serde(deserialize_with = "required_nullable")]
    pub path: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub icon: Option<String>,
    pub sort_order: i64,
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<MicroMenuItem>>,
    #[serde(flatten)]
    pub additional: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Public wire type generated from `$defs.MicroApplication`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroApplication {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<MicroRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_type: Option<MicroEntryType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_url: Option<String>,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub deployment_version: OptionalNullable<String>,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub entry_version_id: OptionalNullable<String>,
}

/// Public wire type generated from `$defs.AppAuthorizationContext`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppAuthorizationContext {
    pub contract_version: ContextContractVersion,
    pub context_version: String,
    pub expires_at: String,
    pub app_code: String,
    pub application: MicroApplication,
    pub user: MicroUser,
    pub tenant: MicroTenant,
    pub organization: MicroOrganization,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub organization_scope_ids: OptionalNullable<Vec<String>>,
    pub roles: Vec<MicroRole>,
    pub permissions: Vec<String>,
    pub data_grants: Vec<MicroDataGrant>,
    pub menus: Vec<MicroMenuItem>,
}

/// Public wire type generated from `$defs.MicroPropsWire`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroPropsWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_version: Option<SDKVersion>,
    pub contract_version: MicroPropsContractVersion,
    pub context_version: String,
    pub context_epoch: i64,
    pub expires_at: String,
    pub token_expires_at: String,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub deployment_version: OptionalNullable<String>,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub entry_version_id: OptionalNullable<String>,
    pub app_code: String,
    pub base_path: String,
    pub route_path: String,
    pub token: String,
    pub user: MicroUser,
    pub tenant: MicroTenant,
    pub organization: MicroOrganization,
    #[serde(deserialize_with = "required_nullable")]
    pub organization_scope_ids: Option<Vec<String>>,
    pub roles: Vec<MicroRole>,
    pub permissions: Vec<String>,
    pub data_grants: Vec<MicroDataGrant>,
    pub theme: MicroTheme,
    pub menus: Vec<MicroMenuItem>,
}

/// Public wire type generated from `$defs.NotificationTarget`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum NotificationTarget {
    #[serde(rename = "USER")]
    User {
        #[serde(rename = "userId")]
        user_id: String,
        #[serde(rename = "organizationId")]
        organization_id: String,
    },
    #[serde(rename = "ORGANIZATION")]
    Organization {
        #[serde(rename = "organizationId")]
        organization_id: String,
    },
    #[serde(rename = "TENANT")]
    Tenant {},
}

/// Public wire type generated from `$defs.NotificationPublishBody`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationPublishBody {
    pub idempotency_key: String,
    pub target: NotificationTarget,
    pub r#type: NotificationType,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// Public wire type generated from `$defs.NotificationPublication`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPublication {
    pub publication_id: String,
    pub recipient_count: i64,
    pub delivery_count: i64,
    pub duplicate: bool,
    pub created_at: String,
}

/// Public wire type generated from `$defs.NotificationSocketContext`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSocketContext {
    pub contract_version: NotificationContractVersion,
    pub user_id: String,
    pub tenant_id: String,
    pub organization_id: String,
    pub expires_at: String,
}

/// Public wire type generated from `$defs.NotificationPlatformSocketContext`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPlatformSocketContext {
    pub contract_version: NotificationContractVersion,
    pub user_id: String,
    pub tenant_id: String,
    pub organization_id: String,
    pub session_id: String,
    pub expires_at: String,
}

/// Public wire type generated from `$defs.RealtimeNotification`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeNotification {
    pub contract_version: NotificationContractVersion,
    pub id: String,
    pub user_id: String,
    pub tenant_id: String,
    pub organization_id: String,
    #[serde(deserialize_with = "required_nullable")]
    pub application_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub publication_id: Option<String>,
    pub r#type: NotificationType,
    pub title: String,
    pub content: String,
    pub link: String,
    pub created_at: String,
}

/// Public wire type generated from `$defs.NotificationRecord`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub id: String,
    pub user_id: String,
    pub tenant_id: String,
    pub organization_id: String,
    pub source_kind: NotificationSourceKind,
    #[serde(deserialize_with = "required_nullable")]
    pub application_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub publication_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub application_name: Option<String>,
    pub r#type: NotificationType,
    pub title: String,
    pub content: String,
    pub link: String,
    pub is_read: bool,
    #[serde(deserialize_with = "required_nullable")]
    pub read_at: Option<String>,
    pub created_at: String,
}

/// Public wire type generated from `$defs.NotificationSourceKind`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationSourceKind {
    #[serde(rename = "APP")]
    App,
    #[serde(rename = "SYSTEM")]
    System,
}

/// Public wire type generated from `$defs.PublicRequestErrorKind`; validation is separate from authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicRequestErrorKind {
    #[serde(rename = "UNAUTHORIZED")]
    Unauthorized,
    #[serde(rename = "FORBIDDEN")]
    Forbidden,
    #[serde(rename = "CONFLICT")]
    Conflict,
    #[serde(rename = "NETWORK")]
    Network,
    #[serde(rename = "PROTOCOL")]
    Protocol,
    #[serde(rename = "HTTP")]
    Http,
}

/// Public wire type generated from `$defs.PublicRequestError`; validation is separate from authorization.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicRequestError {
    pub code: String,
    pub message: String,
    pub kind: PublicRequestErrorKind,
    pub status: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

// A required nullable field must reject absence instead of silently accepting None.
fn required_nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

/// Three wire states for an optional nullable field. Missing is the safe legacy fallback.
#[derive(Clone, Default, PartialEq)]
pub enum OptionalNullable<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}
impl<T> OptionalNullable<T> {
    /// Used only by serde to omit a field that was absent on the wire.
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}
impl<T: Serialize> Serialize for OptionalNullable<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(value) => value.serialize(serializer),
            Self::Null | Self::Missing => serializer.serialize_none(),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for OptionalNullable<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(|value| value.map_or(Self::Null, Self::Value))
    }
}

/// Fixed, credential-free public protocol failure. It contains no rejected payload or source error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolError;
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PUBLIC_CONTRACT_INVALID")
    }
}
impl std::error::Error for ProtocolError {}

fn organization_ceiling<'a>(
    scope: &'a OptionalNullable<Vec<String>>,
    current: &'a str,
) -> Result<Option<Vec<&'a str>>, ProtocolError> {
    match scope {
        OptionalNullable::Missing => Ok(Some(vec![current])),
        OptionalNullable::Null => Ok(None),
        OptionalNullable::Value(ids)
            if ids.len() <= ORGANIZATION_LIMIT
                && ids.iter().all(|id| !id.is_empty())
                && ids.iter().any(|id| id == current) =>
        {
            Ok(Some(ids.iter().map(String::as_str).collect()))
        }
        OptionalNullable::Value(_) => Err(ProtocolError),
    }
}
impl AppAuthorizationContext {
    /// Return the hard organization ceiling; absence restricts an older 1.x context to its current organization.
    /// Explicit null is trusted only after the consumer has verified the platform response and identity binding.
    pub fn organization_ceiling(&self) -> Result<Option<Vec<&str>>, ProtocolError> {
        organization_ceiling(&self.organization_scope_ids, &self.organization.id)
    }
    /// Validate required identifiers and the hard ceiling. The caller must still check expiry, app audience,
    /// introspection binding, permissions and record ownership; this function never authorizes a request.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.app_code.is_empty()
            || self.context_version.is_empty()
            || self.application.id.is_empty()
            || self.user.id.is_empty()
            || self.tenant.id.is_empty()
            || self.organization.id.is_empty()
        {
            return Err(ProtocolError);
        }
        self.organization_ceiling()?;
        if self
            .data_grants
            .iter()
            .any(|grant| grant.organization_ids.len() > ORGANIZATION_LIMIT)
        {
            return Err(ProtocolError);
        }
        Ok(())
    }
    /// Read a bounded public context without logging or retaining parser errors.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > RESPONSE_LIMIT_BYTES {
            return Err(ProtocolError);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ProtocolError)?;
        value.validate()?;
        Ok(value)
    }
}
impl MicroPropsWire {
    /// Validate wire-only bounds; browsers additionally require an unexpired host-owned signal and token lifetime.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if !(1..=9007199254740991).contains(&self.context_epoch)
            || self.context_version.is_empty()
            || self.token.is_empty()
            || self.app_code.is_empty()
            || self.base_path != format!("/micro/{}", self.app_code)
        {
            return Err(ProtocolError);
        }
        if let Some(ids) = &self.organization_scope_ids {
            if ids.len() > ORGANIZATION_LIMIT
                || !ids.iter().any(|id| id == &self.organization.id)
                || ids.iter().any(String::is_empty)
            {
                return Err(ProtocolError);
            }
        }
        Ok(())
    }
    /// Parse only a bounded serialized props DTO. This is not an SDK token refresh or an authorization decision.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > RESPONSE_LIMIT_BYTES {
            return Err(ProtocolError);
        }
        let raw: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ProtocolError)?;
        if raw.get("organizationScopeIds").is_none() {
            return Err(ProtocolError);
        }
        let value: Self = serde_json::from_value(raw).map_err(|_| ProtocolError)?;
        value.validate()?;
        Ok(value)
    }
}

fn notification_text(title: &str, content: &str, link: &str) -> bool {
    !title.trim().is_empty()
        && title.chars().count() <= 200
        && content.chars().count() <= 8192
        && link.chars().count() <= 300
}
fn notification_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128
}
fn publication_pair(application: &Option<String>, publication: &Option<String>) -> bool {
    match (application, publication) {
        (None, None) => true,
        (Some(application), Some(publication)) => {
            notification_id(application) && notification_id(publication)
        }
        _ => false,
    }
}
impl NotificationPublishBody {
    /// Check public input bounds only. The platform separately validates authorization and the link's application owner.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let key = &self.idempotency_key;
        if key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
            || !notification_text(
                &self.title,
                self.content.as_deref().unwrap_or(""),
                self.link.as_deref().unwrap_or(""),
            )
        {
            return Err(ProtocolError);
        }
        let target = match &self.target {
            NotificationTarget::User {
                user_id,
                organization_id,
            } => notification_id(user_id) && notification_id(organization_id),
            NotificationTarget::Organization { organization_id } => {
                notification_id(organization_id)
            }
            NotificationTarget::Tenant {} => true,
        };
        if !target {
            return Err(ProtocolError);
        }
        Ok(())
    }
    /// Parse a bounded publish DTO without exposing parser input in its failure.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > RESPONSE_LIMIT_BYTES {
            return Err(ProtocolError);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ProtocolError)?;
        value.validate()?;
        Ok(value)
    }
}
impl RealtimeNotification {
    /// Validate recipient and publication shape; the receiving session still supplies the authoritative identity.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if ![
            &self.id,
            &self.user_id,
            &self.tenant_id,
            &self.organization_id,
        ]
        .into_iter()
        .all(|id| notification_id(id))
            || !notification_text(&self.title, &self.content, &self.link)
            || !publication_pair(&self.application_id, &self.publication_id)
        {
            return Err(ProtocolError);
        }
        Ok(())
    }
}
impl NotificationRecord {
    /// Keep APP publication identity coherent and SYSTEM rows unbound; legacy rows have no current inbox DTO.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let owner = match self.source_kind {
            NotificationSourceKind::App => {
                self.application_id.is_some() && self.publication_id.is_some()
            }
            NotificationSourceKind::System => {
                self.application_id.is_none() && self.publication_id.is_none()
            }
        };
        if !owner
            || ![
                &self.id,
                &self.user_id,
                &self.tenant_id,
                &self.organization_id,
            ]
            .into_iter()
            .all(|id| notification_id(id))
            || !notification_text(&self.title, &self.content, &self.link)
            || !publication_pair(&self.application_id, &self.publication_id)
        {
            return Err(ProtocolError);
        }
        Ok(())
    }
}
