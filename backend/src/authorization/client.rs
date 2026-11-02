use std::{io::Read, sync::Arc, time::Duration};

use reqwest::{
    header::{ACCEPT, CONTENT_TYPE},
    Client, RequestBuilder, StatusCode,
};
use serde_json::Value;

use crate::{
    config::PlatformConfig,
    error::{ApiError, ConfigError, ServiceError, ServiceFailure},
};

use super::{Action, MemberListQuery};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResponseKind {
    CombinedAuthorization,
    MemberList,
    SingleMember,
    Reminder,
    ReminderResult,
}

const MAX_BODY: usize = 256 * 1024;
const MAX_CA: u64 = 1024 * 1024;

#[derive(Clone)]
pub(super) struct PlatformClient {
    http: Client,
    settings: Arc<PlatformConfig>,
}

impl PlatformClient {
    pub(super) fn new(settings: &PlatformConfig) -> Result<Self, ServiceError> {
        let build = || -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
            let mut builder = Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                // reqwest's default protocol-NACK policy may replay requests.
                // Event retries require the caller's original durable identity.
                .retry(reqwest::retry::never())
                .connect_timeout(settings.timeout.min(Duration::from_secs(1)))
                .timeout(settings.timeout)
                .pool_max_idle_per_host(10)
                .pool_idle_timeout(Duration::from_secs(30))
                .no_proxy();
            if let Some(path) = &settings.ca_cert_file {
                let mut bytes = Vec::new();
                std::fs::File::open(path)?
                    .take(MAX_CA + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_CA {
                    return Err(Box::new(ConfigError::PlatformCa));
                }
                let certificates = reqwest::Certificate::from_pem_bundle(&bytes)?;
                if certificates.is_empty() {
                    return Err(Box::new(ConfigError::PlatformCa));
                }
                for certificate in certificates {
                    builder = builder.add_root_certificate(certificate);
                }
            }
            Ok(builder.build()?)
        };
        let http = build().map_err(|source| {
            ServiceError::caused_by(
                ServiceFailure::PlatformClient,
                SanitizedClientSource(source),
            )
        })?;
        Ok(Self {
            http,
            settings: Arc::new(settings.clone()),
        })
    }

    pub(super) async fn authorization(
        &self,
        token: &str,
        resource: &str,
        action: &str,
        request_id: &str,
    ) -> Result<Value, ApiError> {
        let url = self
            .settings
            .origin
            .join("/api/apps/work-orders/authorize")
            .map_err(|_| ApiError::platform_unavailable())?;
        let value = self
            .json(
                self.http
                    .post(url)
                    .basic_auth(&self.settings.client_id, Some(&self.settings.client_secret))
                    .form(&[("token", token), ("resource", resource), ("action", action)]),
                request_id,
                ResponseKind::CombinedAuthorization,
            )
            .await?;
        Ok(value)
    }

    pub(super) fn identity_issuer(&self) -> Option<&str> {
        self.settings.identity_issuer.as_deref()
    }
    pub(super) fn required_scope(&self, action: Action) -> Option<&str> {
        self.settings
            .scope_bindings
            .get(&format!("ticket:{}", action.as_str()))
            .map(String::as_str)
    }

    pub(super) fn notification_scope(&self) -> Option<&str> {
        self.settings
            .scope_bindings
            .get("notification:publish")
            .map(String::as_str)
    }

    pub(super) async fn members(
        &self,
        token: &str,
        query: &MemberListQuery<'_>,
        request_id: &str,
    ) -> Result<(Value, std::time::Instant), ApiError> {
        let url = self
            .settings
            .origin
            .join("/internal/platform/apps/work-orders/members")
            .map_err(|_| ApiError::platform_unavailable())?;
        self.directory(
            self.http
                .post(url)
                .basic_auth(&self.settings.client_id, Some(&self.settings.client_secret))
                .form(&[
                    ("token", token.to_owned()),
                    ("resource", "ticket".to_owned()),
                    ("action", query.action.as_str().to_owned()),
                    ("organizationId", query.organization.to_string()),
                    ("page", query.page.to_string()),
                    ("size", query.size.to_string()),
                    ("q", query.q.to_owned()),
                ]),
            request_id,
            ResponseKind::MemberList,
        )
        .await
    }

    pub(super) async fn member(
        &self,
        token: &str,
        organization: uuid::Uuid,
        user: uuid::Uuid,
        action: Action,
        request_id: &str,
    ) -> Result<(Value, std::time::Instant), ApiError> {
        let url = self
            .settings
            .origin
            .join(&format!(
                "/internal/platform/apps/work-orders/members/{user}"
            ))
            .map_err(|_| ApiError::platform_unavailable())?;
        self.directory(
            self.http
                .post(url)
                .basic_auth(&self.settings.client_id, Some(&self.settings.client_secret))
                .form(&[
                    ("token", token.to_owned()),
                    ("resource", "ticket".to_owned()),
                    ("action", action.as_str().to_owned()),
                    ("organizationId", organization.to_string()),
                ]),
            request_id,
            ResponseKind::SingleMember,
        )
        .await
    }

    pub(super) async fn publish_reminder(
        &self,
        token: &str,
        json: &str,
        request_id: &str,
        attempt: &crate::reminders::Attempt,
    ) -> Result<crate::reminders::Publication, ApiError> {
        let started = std::time::Instant::now();
        // A retry first asks for the original durable result. Only a precise
        // authenticated NOT_FOUND permits publication of the same request.
        let previous = self
            .response_body(
                self.notification_request("result", token, json)?,
                request_id,
                ResponseKind::ReminderResult,
                None,
            )
            .await;
        match previous {
            Ok(bytes) => return decode_notification(&bytes, started, self.settings.timeout),
            Err(error) if error.code() == "NOTIFICATION_RESULT_NOT_FOUND" => {}
            Err(error) => return Err(error),
        }
        let bytes = self
            .response_body(
                self.notification_request("publish", token, json)?,
                request_id,
                ResponseKind::Reminder,
                Some(attempt),
            )
            .await?;
        decode_notification(&bytes, started, self.settings.timeout)
    }

    fn notification_request(
        &self,
        operation: &str,
        token: &str,
        json: &str,
    ) -> Result<RequestBuilder, ApiError> {
        let path = match operation {
            "publish" => "/internal/platform/apps/work-orders/notifications/publish",
            "result" => "/internal/platform/apps/work-orders/notifications/result",
            _ => return Err(crate::reminders::preparation_unknown()),
        };
        let url = self
            .settings
            .origin
            .join(path)
            .map_err(|_| crate::reminders::preparation_unknown())?;
        Ok(self
            .http
            .post(url)
            .basic_auth(&self.settings.client_id, Some(&self.settings.client_secret))
            .form(&[
                ("token", token),
                ("resource", "notification"),
                ("action", "publish"),
                ("request", json),
            ]))
    }

    async fn directory(
        &self,
        request: RequestBuilder,
        request_id: &str,
        kind: ResponseKind,
    ) -> Result<(Value, std::time::Instant), ApiError> {
        let started = std::time::Instant::now();
        let value = self.json(request, request_id, kind).await?;
        decode_directory(value, started, self.settings.timeout)
    }

    async fn json(
        &self,
        request: RequestBuilder,
        request_id: &str,
        kind: ResponseKind,
    ) -> Result<Value, ApiError> {
        let body = self.response_body(request, request_id, kind, None).await?;
        serde_json::from_slice(&body).map_err(|_| failure(kind))
    }

    async fn response_body(
        &self,
        request: RequestBuilder,
        request_id: &str,
        kind: ResponseKind,
        attempt: Option<&crate::reminders::Attempt>,
    ) -> Result<Vec<u8>, ApiError> {
        let timer = crate::telemetry::registry().timer(operational_telemetry::Stage::PlatformHttp);
        let result = self
            .response_body_inner(request, request_id, kind, attempt)
            .await;
        timer.finish(crate::telemetry::outcome(&result));
        result
    }

    async fn response_body_inner(
        &self,
        mut request: RequestBuilder,
        request_id: &str,
        kind: ResponseKind,
        attempt: Option<&crate::reminders::Attempt>,
    ) -> Result<Vec<u8>, ApiError> {
        // Accept metadata only after canonicalization; never reflect arbitrary
        // caller input into upstream headers or an HTTP-client error diagnostic.
        let request_id = crate::uuid_v7::parse(request_id).ok_or_else(|| failure(kind))?;
        request = request.header("x-request-id", request_id.to_string());
        if let Some(attempt) = attempt {
            attempt.mark();
        }
        let mut response = request
            .header(ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| failure(kind))?;
        let authorization_denial = response.status() == StatusCode::FORBIDDEN
            && kind == ResponseKind::CombinedAuthorization;
        let result_missing =
            response.status() == StatusCode::NOT_FOUND && kind == ResponseKind::ReminderResult;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::NOT_FOUND if result_missing => {}
            StatusCode::UNAUTHORIZED => return Err(ApiError::invalid_token()),
            StatusCode::FORBIDDEN if !authorization_denial => return Err(ApiError::data_denied()),
            StatusCode::FORBIDDEN => {}
            StatusCode::NOT_FOUND if kind == ResponseKind::SingleMember => {
                return Err(ApiError::new(
                    404,
                    "MEMBER_NOT_FOUND",
                    "处理人不存在或不属于该组织的有效成员",
                ))
            }
            StatusCode::NOT_FOUND if kind == ResponseKind::Reminder => {
                return Err(ApiError::new(
                    404,
                    "RECIPIENT_NOT_FOUND",
                    "通知接收人不存在或不在当前授权范围内",
                ))
            }
            StatusCode::CONFLICT
                if matches!(kind, ResponseKind::Reminder | ResponseKind::ReminderResult) =>
            {
                return Err(crate::reminders::conflict())
            }
            StatusCode::BAD_REQUEST if !matches!(kind, ResponseKind::CombinedAuthorization) => {
                return Err(ApiError::validation())
            }
            _ => return Err(failure(kind)),
        }
        if !response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|header| header.to_str().ok())
            .and_then(|header| header.split(';').next())
            .is_some_and(|media| media.eq_ignore_ascii_case("application/json"))
            || response
                .content_length()
                .is_some_and(|length| length > MAX_BODY as u64)
        {
            return Err(failure(kind));
        }
        let mut body = Vec::with_capacity(4096);
        while let Some(chunk) = response.chunk().await.map_err(|_| failure(kind))? {
            if chunk.len() > MAX_BODY - body.len() {
                return Err(failure(kind));
            }
            body.extend_from_slice(&chunk);
        }
        if result_missing {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Missing {
                code: String,
                #[serde(default, rename = "message")]
                _message: Option<String>,
            }
            let missing: Missing = serde_json::from_slice(&body).map_err(|_| failure(kind))?;
            if missing.code != "NOT_FOUND" {
                return Err(failure(kind));
            }
            return Err(ApiError::new(
                404,
                "NOTIFICATION_RESULT_NOT_FOUND",
                "原提醒发布结果尚未建立",
            ));
        }
        if authorization_denial {
            let value: Value = serde_json::from_slice(&body).map_err(|_| failure(kind))?;
            return Err(match value["code"].as_str() {
                Some("OPERATION_DENIED") => super::policy::operation_denied(),
                Some("FORBIDDEN") => ApiError::data_denied(),
                _ => failure(kind),
            });
        }
        Ok(body)
    }
}

fn decode_notification(
    bytes: &[u8],
    started: std::time::Instant,
    timeout: Duration,
) -> Result<crate::reminders::Publication, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Reply {
        contract_version: String,
        publication: crate::reminders::Publication,
        expires_at: String,
    }
    let reply: Reply =
        serde_json::from_slice(bytes).map_err(|_| crate::reminders::result_unknown())?;
    let now = chrono::Utc::now().timestamp_millis();
    let expiry = chrono::DateTime::parse_from_rfc3339(&reply.expires_at)
        .map_err(|_| crate::reminders::result_unknown())?
        .timestamp_millis();
    if reply.contract_version != "resource-notification/1"
        || expiry <= now
        || expiry > now.saturating_add(3000)
        || std::time::Instant::now() >= started + timeout.min(Duration::from_secs(3))
    {
        return Err(crate::reminders::result_unknown());
    }
    let publication =
        serde_json::to_vec(&reply.publication).map_err(|_| crate::reminders::result_unknown())?;
    crate::reminders::Publication::from_bytes(&publication)
}

fn decode_directory(
    value: Value,
    started: std::time::Instant,
    timeout: Duration,
) -> Result<(Value, std::time::Instant), ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Reply {
        contract_version: String,
        directory: String,
        expires_at: String,
        data: Value,
    }
    let reply: Reply =
        serde_json::from_value(value).map_err(|_| ApiError::platform_unavailable())?;
    let received = std::time::Instant::now();
    let now = chrono::Utc::now().timestamp_millis();
    let expires = chrono::DateTime::parse_from_rfc3339(&reply.expires_at)
        .map_err(|_| ApiError::platform_unavailable())?
        .timestamp_millis();
    if reply.contract_version != "resource-directory/1"
        || reply.directory != "members"
        || !reply.data.is_object()
        || expires <= now
        || expires > now.saturating_add(3000)
    {
        return Err(ApiError::platform_unavailable());
    }
    let deadline = (received + Duration::from_millis((expires - now) as u64))
        .min(started + timeout.min(Duration::from_secs(3)));
    if std::time::Instant::now() >= deadline {
        return Err(ApiError::platform_unavailable());
    }
    Ok((reply.data, deadline))
}

fn failure(kind: ResponseKind) -> ApiError {
    if matches!(kind, ResponseKind::Reminder | ResponseKind::ReminderResult) {
        crate::reminders::result_unknown()
    } else {
        ApiError::platform_unavailable()
    }
}

// Preserve the private cause for controlled diagnostics; normal formatting can
// never render a local CA path, upstream URL, Basic credentials or bearer token.
struct SanitizedClientSource(Box<dyn std::error::Error + Send + Sync>);
impl std::fmt::Debug for SanitizedClientSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("platform client setup error")
    }
}
impl std::fmt::Display for SanitizedClientSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("platform client setup error")
    }
}
impl std::error::Error for SanitizedClientSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[cfg(test)]
mod directory_tests {
    use super::*;
    use serde_json::json;
    fn reply(offset: i64) -> Value {
        json!({"contractVersion":"resource-directory/1","directory":"members",
            "expiresAt":chrono::DateTime::from_timestamp_millis(chrono::Utc::now().timestamp_millis()+offset).unwrap().to_rfc3339(),
            "data":{"records":[],"total":0,"page":1,"size":20}})
    }
    #[test]
    fn private_directory_wrapper_rejects_wrong_contract_kind_shape_and_deadline() {
        let started = std::time::Instant::now();
        for offset in [-1, 4000] {
            assert!(decode_directory(reply(offset), started, Duration::from_secs(3)).is_err());
        }
        for (field, value) in [
            ("contractVersion", json!("1.0")),
            ("directory", json!("organizations")),
            ("data", json!([])),
            ("extra", Value::Null),
        ] {
            let mut v = reply(2000);
            v[field] = value;
            assert!(
                decode_directory(v, started, Duration::from_secs(3)).is_err(),
                "{field}"
            );
        }
    }
    #[test]
    fn directory_response_cannot_renew_the_original_pre_http_window() {
        let started = std::time::Instant::now() - Duration::from_millis(2500);
        let (_, deadline) = decode_directory(reply(2000), started, Duration::from_secs(3)).unwrap();
        assert!(deadline <= started + Duration::from_secs(3));
        assert!(deadline <= std::time::Instant::now() + Duration::from_millis(500));
        assert!(decode_directory(
            reply(2000),
            started - Duration::from_secs(1),
            Duration::from_secs(3)
        )
        .is_err());
    }
}

#[cfg(test)]
mod notification_tests {
    use super::*;
    use serde_json::json;
    fn reply(offset: i64) -> Value {
        json!({"contractVersion":"resource-notification/1",
            "expiresAt":chrono::DateTime::from_timestamp_millis(chrono::Utc::now().timestamp_millis()+offset).unwrap().to_rfc3339(),
            "publication":{"publicationId":crate::uuid_v7::fixture(1).to_string(),"recipientCount":1,
                "deliveryCount":1,"duplicate":false,"createdAt":"2026-10-28T00:00:00Z"}})
    }
    #[test]
    fn notification_wrapper_cannot_extend_the_original_window_or_weaken_the_publication() {
        let started = std::time::Instant::now();
        assert!(decode_notification(
            &serde_json::to_vec(&reply(2000)).unwrap(),
            started,
            Duration::from_secs(3)
        )
        .is_ok());
        for offset in [-1, 4000] {
            assert!(decode_notification(
                &serde_json::to_vec(&reply(offset)).unwrap(),
                started,
                Duration::from_secs(3)
            )
            .is_err());
        }
        assert!(decode_notification(
            &serde_json::to_vec(&reply(2000)).unwrap(),
            started - Duration::from_secs(4),
            Duration::from_secs(3)
        )
        .is_err());
        for (path, bad) in [
            ("contractVersion", json!("1.0")),
            ("publication", json!([])),
            ("extra", Value::Null),
        ] {
            let mut value = reply(2000);
            value[path] = bad;
            assert!(decode_notification(
                &serde_json::to_vec(&value).unwrap(),
                started,
                Duration::from_secs(3)
            )
            .is_err());
        }
        let mut value = reply(2000);
        value["publication"]["recipientCount"] = json!(2);
        assert!(decode_notification(
            &serde_json::to_vec(&value).unwrap(),
            started,
            Duration::from_secs(3)
        )
        .is_err());
    }
}
