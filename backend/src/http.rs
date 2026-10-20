use std::time::{Duration, Instant};

use axum::{
    body::Bytes,
    extract::{rejection::PathRejection, DefaultBodyLimit, FromRequest, Path, Request, State},
    http::{header::HeaderName, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    authorization::{Action, Authorizer},
    error::{ApiError, ServiceError},
    Config,
};

#[derive(Clone)]
struct HealthState {
    pool: PgPool,
    timeout: Duration,
    authorizer: Option<Authorizer>,
    allowed_origins: Vec<String>,
    metrics_token: Option<String>,
}

#[derive(Clone)]
struct RequestId(String);

#[derive(Serialize)]
struct Health {
    status: &'static str,
    service: &'static str,
    scope: &'static str,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
}

/// Construct the baseline HTTP surface for an already initialized business pool.
///
/// The caller must first run [`crate::initialize_database`]. Liveness describes
/// only this process; readiness checks the independent database, migration
/// histories, and required objects. It does not claim platform authorization or
/// complete business API readiness. All business routes remain unavailable.
pub fn router(pool: PgPool, health_timeout: Duration) -> Router {
    configured_router(pool, health_timeout, None, vec!["null".into()])
}

/// Construct authenticated read, export and transactional write routes.
///
/// Call [`crate::initialize_database`] first. Unlike [`router`]'s compatibility
/// health surface, this requires complete validated platform client settings.
/// Each protected request obtains fresh introspection and application context;
/// the client secret remains in the private server-side client.
pub fn application(config: &Config, pool: PgPool) -> Result<Router, ServiceError> {
    let authorizer = Authorizer::new(config)?;
    Ok(configured_metrics_router(
        pool,
        config.health_timeout,
        Some(authorizer),
        config.allowed_origins.clone(),
        config.metrics_token.clone(),
    ))
}

pub(crate) fn configured_router(
    pool: PgPool,
    timeout: Duration,
    authorizer: Option<Authorizer>,
    allowed_origins: Vec<String>,
) -> Router {
    configured_metrics_router(pool, timeout, authorizer, allowed_origins, None)
}

pub(crate) fn configured_metrics_router(
    pool: PgPool,
    timeout: Duration,
    authorizer: Option<Authorizer>,
    allowed_origins: Vec<String>,
    metrics_token: Option<String>,
) -> Router {
    let state = HealthState {
        metrics_token,
        pool,
        timeout,
        authorizer,
        allowed_origins,
    };
    Router::new()
        .route("/work-orders-api/internal/metrics", get(private_metrics))
        .route("/work-orders-api/health/live", get(live))
        .route("/work-orders-api/health/ready", get(ready))
        .route("/work-orders-api/health", get(ready))
        .route(
            "/work-orders-api/tickets",
            get(list)
                .post(create)
                .patch(business_unavailable)
                .delete(business_unavailable),
        )
        .route("/work-orders-api/tickets/export.csv", get(export))
        .route(
            "/work-orders-api/tickets/{ticket_id}",
            get(detail)
                .post(business_unavailable)
                .patch(update)
                .delete(delete),
        )
        .route("/work-orders-api/tickets/{ticket_id}/remind", post(remind))
        .route("/work-orders-api/assignees", get(assignees))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(65_536))
        .layer(middleware::from_fn_with_state(state, request_metadata))
}

fn bearer(headers: &HeaderMap) -> Result<&str, ApiError> {
    let mut values = headers.get_all("authorization").iter();
    let value = values
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or_else(ApiError::invalid_token)?;
    if values.next().is_some() || value.len() > 8192 {
        return Err(ApiError::invalid_token());
    }
    let token = value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty() && !token.chars().any(char::is_whitespace));
    token.ok_or_else(ApiError::invalid_token)
}

async fn list(
    State(state): State<HealthState>,
    Extension(id): Extension<RequestId>,
    request: Request,
) -> Result<Response, ApiError> {
    let Some(authorizer) = &state.authorizer else {
        return Ok(business_unavailable().await);
    };
    let auth = authorizer
        .authorize(bearer(request.headers())?, Action::Read, &id.0)
        .await?;
    Ok(Json(crate::reads::list(&state.pool, &auth, request.uri().query()).await?).into_response())
}

async fn detail(
    State(state): State<HealthState>,
    Extension(id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> Result<Response, ApiError> {
    let Some(authorizer) = &state.authorizer else {
        return Ok(business_unavailable().await);
    };
    let auth = authorizer
        .authorize(bearer(request.headers())?, Action::Read, &id.0)
        .await?;
    let Path(ticket_id) = path.map_err(|_| ApiError::validation())?;
    if request.uri().query().is_some_and(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .next()
            .is_some()
    }) {
        return Err(ApiError::validation());
    }
    Ok(Json(crate::reads::detail(&state.pool, &auth, &ticket_id).await?).into_response())
}

async fn export(
    State(state): State<HealthState>,
    Extension(id): Extension<RequestId>,
    request: Request,
) -> Result<Response, ApiError> {
    let Some(authorizer) = &state.authorizer else {
        return Ok(business_unavailable().await);
    };
    let auth = authorizer
        .authorize(bearer(request.headers())?, Action::Export, &id.0)
        .await?;
    let bytes = crate::reads::export(&state.pool, &auth, request.uri().query()).await?;
    let mut response = Response::new(axum::body::Body::from(bytes));
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response.headers_mut().insert(
        "content-disposition",
        HeaderValue::from_static("attachment; filename=\"work-orders.csv\""),
    );
    Ok(response)
}

async fn assignees(
    State(state): State<HealthState>,
    Extension(id): Extension<RequestId>,
    request: Request,
) -> Result<Response, ApiError> {
    let Some(authorizer) = &state.authorizer else {
        return Ok(business_unavailable().await);
    };
    let result = crate::directory::list(
        authorizer,
        bearer(request.headers())?,
        &id.0,
        request.uri().query(),
    )
    .await?;
    Ok(Json(result).into_response())
}

fn no_query(request: &Request) -> Result<(), ApiError> {
    if request.uri().query().is_some_and(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .next()
            .is_some()
    }) {
        Err(ApiError::validation())
    } else {
        Ok(())
    }
}

async fn write_body(request: Request, state: &HealthState) -> Result<Bytes, ApiError> {
    let content_type = single_header(request.headers(), "content-type")
        .map_err(|_| ApiError::validation())?
        .ok_or_else(ApiError::validation)?;
    if !content_type
        .split(';')
        .next()
        .is_some_and(|value| value.eq_ignore_ascii_case("application/json"))
    {
        return Err(ApiError::validation());
    }
    Bytes::from_request(request, state).await.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(413, "REQUEST_LIMIT", "请求体超过 64 KiB")
        } else {
            ApiError::validation()
        }
    })
}

async fn create(
    State(state): State<HealthState>,
    Extension(id): Extension<RequestId>,
    request: Request,
) -> Result<Response, ApiError> {
    let Some(authorizer) = &state.authorizer else {
        return Ok(business_unavailable().await);
    };
    let token = bearer(request.headers())?.to_owned();
    let auth = authorizer.authorize(&token, Action::Create, &id.0).await?;
    no_query(&request)?;
    let key = single_header(request.headers(), "idempotency-key")
        .map_err(|_| ApiError::validation())?
        .ok_or_else(ApiError::validation)?
        .to_owned();
    let body = write_body(request, &state).await?;
    let ticket =
        crate::writes::create(&state.pool, &auth, &key, &body, authorizer, &token, &id.0).await?;
    Ok((StatusCode::CREATED, Json(ticket)).into_response())
}

async fn update(
    State(state): State<HealthState>,
    Extension(id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> Result<Response, ApiError> {
    let Some(authorizer) = &state.authorizer else {
        return Ok(business_unavailable().await);
    };
    let token = bearer(request.headers())?.to_owned();
    let auth = authorizer.authorize(&token, Action::Update, &id.0).await?;
    let Path(ticket_id) = path.map_err(|_| ApiError::validation())?;
    no_query(&request)?;
    let body = write_body(request, &state).await?;
    Ok(Json(
        crate::writes::update(
            &state.pool,
            &auth,
            &ticket_id,
            &body,
            authorizer,
            &token,
            &id.0,
        )
        .await?,
    )
    .into_response())
}

async fn delete(
    State(state): State<HealthState>,
    Extension(id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> Result<Response, ApiError> {
    let Some(authorizer) = &state.authorizer else {
        return Ok(business_unavailable().await);
    };
    let auth = authorizer
        .authorize(bearer(request.headers())?, Action::Delete, &id.0)
        .await?;
    let Path(ticket_id) = path.map_err(|_| ApiError::validation())?;
    no_query(&request)?;
    let body = write_body(request, &state).await?;
    crate::writes::delete(&state.pool, &auth, &ticket_id, &body).await?;
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}

async fn remind(
    State(state): State<HealthState>,
    Extension(id): Extension<RequestId>,
    Extension(attempt): Extension<crate::reminders::Attempt>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> Result<Response, ApiError> {
    let Some(authorizer) = &state.authorizer else {
        return Ok(business_unavailable().await);
    };
    let token = bearer(request.headers())?.to_owned();
    let auth = authorizer.authorize_reminder(&token, &id.0).await?;
    let Path(ticket_id) = path.map_err(|_| ApiError::validation())?;
    no_query(&request)?;
    let body = write_body(request, &state).await?;
    let result = crate::reminders::send(
        &state.pool,
        &auth,
        authorizer,
        &token,
        &ticket_id,
        &body,
        (&id.0, &attempt),
    )
    .await?;
    Ok(Json(result).into_response())
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error(
            StatusCode::from_u16(self.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            self.code(),
            self.message(),
        )
    }
}

async fn live() -> Json<Health> {
    Json(Health {
        status: "UP",
        service: "work-orders-api",
        scope: "process",
    })
}

async fn ready(State(state): State<HealthState>) -> Response {
    match tokio::time::timeout(
        state.timeout,
        crate::migrations::verify_database(&state.pool),
    )
    .await
    {
        Ok(Ok(())) => Json(Health {
            status: "UP",
            service: "work-orders-api",
            scope: "business-database-schema",
        })
        .into_response(),
        _ => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "BUSINESS_DATABASE_UNAVAILABLE",
            "工单业务数据库或迁移基线不可用",
        ),
    }
}

async fn business_unavailable() -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "BUSINESS_API_UNAVAILABLE",
        "Rust 工单业务接口尚未开放",
    )
}

async fn not_found() -> Response {
    error(
        StatusCode::NOT_FOUND,
        "RESOURCE_NOT_FOUND",
        "请求的资源不存在",
    )
}

async fn method_not_allowed() -> Response {
    error(
        StatusCode::METHOD_NOT_ALLOWED,
        "METHOD_NOT_ALLOWED",
        "此资源不支持该请求方法",
    )
}

fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (status, Json(ErrorBody { code, message })).into_response()
}

fn reminder_path(path: &str) -> bool {
    path.strip_prefix("/work-orders-api/tickets/")
        .and_then(|tail| tail.strip_suffix("/remind"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

async fn request_metadata(
    State(state): State<HealthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_timer = crate::telemetry::registry().timer(operational_telemetry::Stage::Request);
    let request_id = canonical_request_id(request.headers().get_all("x-request-id").iter());
    let route = match request.uri().path() {
        "/work-orders-api/health/live" => "health_live",
        "/work-orders-api/health/ready" | "/work-orders-api/health" => "health_ready",
        "/work-orders-api/tickets" => "tickets",
        "/work-orders-api/tickets/export.csv" => "tickets_export",
        "/work-orders-api/assignees" => "assignees",
        path if reminder_path(path) => "ticket_remind",
        path if path.starts_with("/work-orders-api/tickets/") => "ticket_detail",
        _ => "unmatched",
    };
    let started = Instant::now();
    let reminder_attempt = (request.method() == Method::POST
        && reminder_path(request.uri().path()))
    .then(crate::reminders::Attempt::default);
    if let Some(attempt) = &reminder_attempt {
        request.extensions_mut().insert(attempt.clone());
    }
    // Dropping an in-flight COMMIT cannot establish whether it committed. The
    // caller must retain creation identity or reconcile the versioned mutation.
    let timeout_error =
        if request.method() == Method::POST && request.uri().path() == "/work-orders-api/tickets" {
            ApiError::new(
                503,
                "CREATE_RESULT_UNKNOWN",
                "创建结果尚未确认，请使用相同幂等键和正文重试",
            )
        } else if matches!(*request.method(), Method::PATCH | Method::DELETE)
            && request
                .uri()
                .path()
                .starts_with("/work-orders-api/tickets/")
        {
            ApiError::new(
                503,
                "WRITE_RESULT_UNKNOWN",
                "写入结果尚未确认，请刷新工单核验当前版本",
            )
        } else {
            ApiError::new(503, "REQUEST_TIMEOUT", "业务服务请求超时，请稍后重试")
        };
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));
    let origin = single_header(request.headers(), "origin");
    let allowed_origin = origin
        .as_ref()
        .ok()
        .and_then(|origin| *origin)
        .filter(|origin| {
            state
                .allowed_origins
                .iter()
                .any(|allowed| allowed == origin)
        });
    let origin_rejected = origin.is_err()
        || origin.as_ref().ok().and_then(|origin| *origin).is_some() && allowed_origin.is_none();
    let allowed_origin = allowed_origin.and_then(|origin| HeaderValue::from_str(origin).ok());
    let mut response = if origin_rejected {
        origin_denied()
    } else if request.method() == Method::OPTIONS {
        let method = single_header(request.headers(), "access-control-request-method");
        let headers = single_header(request.headers(), "access-control-request-headers");
        if allowed_origin.is_none()
            || !matches!(method, Ok(Some("GET" | "POST" | "PATCH" | "DELETE")))
            || headers.is_err()
            || headers.ok().flatten().is_some_and(|headers| {
                headers
                    .split(',')
                    .map(str::trim)
                    .filter(|header| !header.is_empty())
                    .any(|header| {
                        ![
                            "authorization",
                            "content-type",
                            "x-request-id",
                            "idempotency-key",
                        ]
                        .contains(&header.to_ascii_lowercase().as_str())
                    })
            })
        {
            origin_denied()
        } else {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert(
                "access-control-allow-methods",
                HeaderValue::from_static("GET, POST, PATCH, DELETE, OPTIONS"),
            );
            response.headers_mut().insert(
                "access-control-allow-headers",
                HeaderValue::from_static(
                    "Authorization, Content-Type, X-Request-Id, Idempotency-Key",
                ),
            );
            response
        }
    } else {
        match tokio::time::timeout(Duration::from_secs(15), next.run(request)).await {
            Ok(response) => response,
            Err(_) => {
                let failure = match &reminder_attempt {
                    Some(attempt) if attempt.attempted() => crate::reminders::result_unknown(),
                    Some(_) => crate::reminders::preparation_unknown(),
                    None => timeout_error,
                };
                failure.into_response()
            }
        }
    };
    if let Some(origin) = allowed_origin {
        response
            .headers_mut()
            .insert("access-control-allow-origin", origin);
        response.headers_mut().insert(
            "access-control-expose-headers",
            HeaderValue::from_static("X-Request-Id"),
        );
    }
    for (name, value) in [
        ("vary", "Origin"),
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        (
            "content-security-policy",
            "default-src 'none'; frame-ancestors 'none'",
        ),
    ] {
        response.headers_mut().insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    // Canonical UUIDs contain only ASCII hex and hyphens, so this conversion
    // cannot fail. Preserve the fallback without panicking in a request path.
    if let Ok(header) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), header);
    }
    tracing::info!(
        event = "work_orders_http_complete",
        route,
        request_id,
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis() as u64,
    );
    request_timer.finish(operational_telemetry::Outcome::http(
        response.status().as_u16(),
    ));
    response
}

fn origin_denied() -> Response {
    ApiError::new(403, "ORIGIN_DENIED", "来源或预检请求未获允许").into_response()
}

fn single_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|value| value.to_str().map_err(|_| ()))
        .transpose()?;
    if values.next().is_some() {
        return Err(());
    }
    Ok(value)
}

fn canonical_request_id<'a>(mut values: impl Iterator<Item = &'a HeaderValue>) -> String {
    let value = values.next().and_then(|header| header.to_str().ok());
    let supplied = if values.next().is_none() {
        value
            .filter(|value| value.len() == 36)
            .and_then(|value| Uuid::parse_str(value).ok())
    } else {
        None
    };
    supplied.unwrap_or_else(Uuid::new_v4).to_string()
}

async fn private_metrics(State(state): State<HealthState>, headers: HeaderMap) -> Response {
    crate::telemetry::metrics(&state.pool, state.metrics_token.as_deref(), &headers).await
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Request};
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    fn app() -> Router {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgresql://worker:secret@127.0.0.1:1/orders")
            .unwrap();
        router(pool, Duration::from_millis(10))
    }

    #[tokio::test]
    async fn create_preflight_allows_the_stable_key_without_authorization_or_database() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/work-orders-api/tickets")
                    .header("origin", "null")
                    .header("access-control-request-method", "POST")
                    .header(
                        "access-control-request-headers",
                        "Authorization, Content-Type, Idempotency-Key",
                    )
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()["access-control-allow-origin"], "null");
        assert!(response.headers()["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .contains("Idempotency-Key"));
        assert!(!response
            .headers()
            .contains_key("access-control-allow-credentials"));
    }

    #[tokio::test]
    async fn live_is_independent_and_business_routes_are_explicitly_unavailable() {
        let live = app()
            .oneshot(
                Request::builder()
                    .uri("/work-orders-api/health/live")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(live.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(live.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["scope"], "process");
        for path in [
            "/work-orders-api/tickets",
            "/work-orders-api/tickets/anything",
            "/work-orders-api/assignees",
        ] {
            let result = app()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(result.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    #[tokio::test]
    async fn readiness_failure_is_bounded_and_sanitized() {
        for path in ["/work-orders-api/health", "/work-orders-api/health/ready"] {
            let result = tokio::time::timeout(
                Duration::from_secs(1),
                app().oneshot(
                    Request::builder()
                        .uri(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(result.status(), StatusCode::SERVICE_UNAVAILABLE);
            let bytes = to_bytes(result.into_body(), 4096).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["code"], "BUSINESS_DATABASE_UNAVAILABLE");
            assert!(!String::from_utf8_lossy(&bytes).contains("secret"));
            assert_eq!(body.as_object().unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn request_id_is_canonical_and_untrusted_paths_do_not_enter_error_text() {
        let id = "550E8400-E29B-41D4-A716-446655440000";
        let result = app()
            .oneshot(
                Request::builder()
                    .uri("/secret-token?credential=secret-token")
                    .header("x-request-id", id)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::NOT_FOUND);
        assert_eq!(result.headers()["x-request-id"], id.to_ascii_lowercase());
        let body = to_bytes(result.into_body(), 4096).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("secret-token"));
        let result = app()
            .oneshot(
                Request::builder()
                    .uri("/work-orders-api/health/live")
                    .header("x-request-id", "secret-token")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(Uuid::parse_str(result.headers()["x-request-id"].to_str().unwrap()).is_ok());
    }

    #[tokio::test]
    async fn wrong_health_method_uses_json_contract() {
        let result = app()
            .oneshot(
                Request::builder()
                    .uri("/work-orders-api/health/live")
                    .method("POST")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(result.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["code"], "METHOD_NOT_ALLOWED");
    }

    #[tokio::test]
    async fn opaque_preflight_does_not_require_database_or_authorization() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/work-orders-api/tickets")
                    .header("origin", "null")
                    .header("access-control-request-method", "GET")
                    .header(
                        "access-control-request-headers",
                        "Authorization, X-Request-Id",
                    )
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()["access-control-allow-origin"], "null");
        assert_eq!(
            response.headers()["access-control-expose-headers"],
            "X-Request-Id"
        );
        assert!(!response
            .headers()
            .contains_key("access-control-allow-credentials"));
        assert!(response.headers().contains_key("x-request-id"));
    }

    #[tokio::test]
    async fn preflight_and_origin_denials_preserve_safe_metadata() {
        for (origin, method, headers, allow_origin) in [
            (Some("null"), "TRACE", "authorization", true),
            (Some("null"), "GET", "x-secret", true),
            (
                Some("https://unregistered.invalid"),
                "GET",
                "authorization",
                false,
            ),
            (None, "GET", "authorization", false),
        ] {
            let mut request = Request::builder()
                .method("OPTIONS")
                .uri("/work-orders-api/tickets")
                .header("access-control-request-method", method)
                .header("access-control-request-headers", headers);
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            let response = app()
                .oneshot(request.body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(
                response
                    .headers()
                    .contains_key("access-control-allow-origin"),
                allow_origin
            );
            assert!(response.headers().contains_key("x-request-id"));
            assert_eq!(response.headers()["cache-control"], "no-store");
            let body: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(body["code"], "ORIGIN_DENIED");
        }
    }

    #[test]
    fn bearer_rejects_duplicate_ambiguous_and_unbounded_credentials() {
        for value in [
            "",
            "Bearer ",
            "bearer value",
            "Bearer two tokens",
            "Bearer\tvalue",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("authorization", HeaderValue::from_str(value).unwrap());
            assert_eq!(bearer(&headers).unwrap_err().status(), 401);
        }
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer opaque"));
        assert_eq!(bearer(&headers).unwrap(), "opaque");
        headers.append("authorization", HeaderValue::from_static("Bearer opaque"));
        assert_eq!(bearer(&headers).unwrap_err().status(), 401);
        headers.clear();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", "x".repeat(8192))).unwrap(),
        );
        assert_eq!(bearer(&headers).unwrap_err().status(), 401);
    }

    #[test]
    fn duplicate_or_noncanonical_ids_are_replaced() {
        let headers = [
            HeaderValue::from_static("550e8400-e29b-41d4-a716-446655440000"),
            HeaderValue::from_static("550e8400-e29b-41d4-a716-446655440000"),
        ];
        let id = canonical_request_id(headers.iter());
        assert_ne!(id, headers[0].to_str().unwrap());
        let headers = [HeaderValue::from_static("550e8400e29b41d4a716446655440000")];
        assert_ne!(
            canonical_request_id(headers.iter()),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }
}
