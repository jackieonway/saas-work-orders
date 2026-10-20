use std::sync::{
    atomic::{AtomicU8, AtomicUsize, Ordering},
    Mutex,
};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use uuid::Uuid;

use super::*;

const USER: &str = "11111111-1111-4111-8111-111111111111";
const TENANT: &str = "22222222-2222-4222-8222-222222222222";
const ORG: &str = "33333333-3333-4333-8333-333333333333";
const CHILD: &str = "44444444-4444-4444-8444-444444444444";
const APP: &str = "55555555-5555-4555-8555-555555555555";
const REQUEST_ID: &str = "66666666-6666-4666-8666-666666666666";

fn intro(now: i64) -> Value {
    json!({"active":true,"aud":"work-orders","sub":USER,"tenant_id":TENANT,
        "organization_id":ORG,"token_type":"Bearer","exp":now/1000+120})
}

fn context(now: i64) -> Value {
    json!({"contractVersion":"1.0","appCode":"work-orders","contextVersion":"a".repeat(64),
        "expiresAt":chrono::DateTime::from_timestamp_millis(now+20_000).unwrap().to_rfc3339(),
        "user":{"id":USER,"username":"reader","displayName":"读者"},
        "tenant":{"id":TENANT},"organization":{"id":ORG,"name":"组织"},"application":{"id":APP},
        "permissions":["ticket:read","ticket:export"],"dataGrants":[
            {"resource":"ticket","action":"read","tenantId":TENANT,"subjectUserId":USER,
                "scope":"SELF","organizationIds":[]},
            {"resource":"ticket","action":"export","tenantId":TENANT,"subjectUserId":USER,
                "scope":"TENANT","organizationIds":[]} ]})
}

fn authorized(ctx: &Value, action: Action, now: i64) -> Result<Authorized, ApiError> {
    let identity = policy::validate_intro(&intro(now), now)?;
    policy::validate_context(&identity, ctx, action, now)
}

#[test]
fn write_actions_require_their_own_grant_and_trusted_creation_snapshots() {
    let now = policy::now_millis();
    for action in [Action::Create, Action::Update, Action::Delete] {
        let mut ctx = context(now);
        ctx["permissions"] = json!([format!("ticket:{}", action.as_str())]);
        ctx["dataGrants"][0]["action"] = json!(action.as_str());
        let auth = authorized(&ctx, action, now).unwrap();
        auth.require_action(action).unwrap();
        assert!(auth.require_action(Action::Read).is_err());
        assert_eq!(auth.organization_name(), "组织");
        assert_eq!(auth.owner_display_name(), "读者");
        ctx["dataGrants"][0]["action"] = json!("read");
        assert_eq!(
            authorized(&ctx, action, now).err().unwrap().code(),
            "DATA_DENIED"
        );
    }
    let mut ctx = context(now);
    ctx["permissions"] = json!(["ticket:create"]);
    ctx["dataGrants"][0]["action"] = json!("create");
    ctx["user"]["displayName"] = json!("  ");
    assert_eq!(
        authorized(&ctx, Action::Create, now)
            .unwrap()
            .owner_display_name(),
        "reader"
    );
    ctx["user"]["username"] = json!("中".repeat(101));
    assert_eq!(
        authorized(&ctx, Action::Create, now).err().unwrap().code(),
        "INVALID_CONTEXT"
    );
}

#[test]
fn creation_and_assignment_targets_obey_scope_and_organization_ceiling() {
    let now = policy::now_millis();
    let current = Uuid::parse_str(ORG).unwrap();
    let child = Uuid::parse_str(CHILD).unwrap();
    let outsider = Uuid::new_v4();
    for (scope, organizations, child_allowed) in [
        ("SELF", json!([]), false),
        ("ORG", json!([ORG]), false),
        ("ORG_SUBTREE", json!([ORG, CHILD]), true),
        ("TENANT", json!([]), true),
    ] {
        let mut ctx = context(now);
        ctx["organizationScopeIds"] = json!([ORG, CHILD]);
        ctx["dataGrants"][0]["scope"] = json!(scope);
        ctx["dataGrants"][0]["organizationIds"] = organizations;
        let auth = authorized(&ctx, Action::Read, now).unwrap();
        auth.require_organization(current).unwrap();
        assert_eq!(auth.require_organization(child).is_ok(), child_allowed);
        assert!(auth.require_organization(outsider).is_err());
    }
}

#[test]
fn introspection_rejects_wrong_audience_activity_type_identity_and_expiry() {
    let now = 1_700_000_000_000;
    for (field, value) in [
        ("active", json!(false)),
        ("active", json!("true")),
        ("aud", json!("other-app")),
        ("aud", json!(["work-orders"])),
        ("token_type", json!("bearer")),
        ("sub", json!("not-a-uuid")),
        ("tenant_id", json!(null)),
        ("organization_id", json!("33333333333343338333333333333333")),
        ("exp", json!(now / 1000)),
        ("exp", json!(1.25)),
        ("exp", json!(9_007_199_254_740_992_u64)),
        ("exp", json!((now / 1000 + 60).to_string())),
    ] {
        let mut candidate = intro(now);
        candidate[field] = value;
        assert_eq!(
            policy::validate_intro(&candidate, now)
                .err()
                .unwrap()
                .code(),
            "INVALID_APP_TOKEN",
            "{field}"
        );
    }
    let mut upper = intro(now);
    upper["sub"] = json!(USER.to_uppercase());
    assert!(policy::validate_intro(&upper, now).is_ok());
}

#[test]
fn context_binds_identity_contract_version_expiry_and_display_fields() {
    let now = 1_700_000_000_000;
    for (pointer, value) in [
        ("/contractVersion", json!("2.0")),
        ("/appCode", json!("other-app")),
        ("/contextVersion", json!("z".repeat(64))),
        ("/contextVersion", json!("a".repeat(63))),
        ("/expiresAt", json!("invalid")),
        (
            "/expiresAt",
            json!(chrono::DateTime::from_timestamp_millis(now)
                .unwrap()
                .to_rfc3339()),
        ),
        (
            "/expiresAt",
            json!(chrono::DateTime::from_timestamp_millis(now + 30_001)
                .unwrap()
                .to_rfc3339()),
        ),
        ("/user/id", json!(CHILD)),
        ("/tenant/id", json!(CHILD)),
        ("/organization/id", json!(CHILD)),
        ("/application/id", json!(null)),
        ("/user/username", json!(false)),
        ("/organization/name", json!("  ")),
        ("/organization/name", json!("中".repeat(101))),
        ("/user/displayName", json!("中".repeat(101))),
        ("/user/displayName", json!({})),
    ] {
        let mut candidate = context(now);
        *candidate.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            authorized(&candidate, Action::Read, now)
                .err()
                .unwrap()
                .code(),
            "INVALID_CONTEXT",
            "{pointer}"
        );
    }
    let mut token = intro(now);
    token["exp"] = json!(now / 1000 + 1);
    let identity = policy::validate_intro(&token, now).unwrap();
    assert_eq!(
        policy::validate_context(&identity, &context(now), Action::Read, now)
            .err()
            .unwrap()
            .code(),
        "INVALID_CONTEXT"
    );
}

#[test]
fn organization_ceiling_is_fail_closed_and_only_explicit_null_is_unbounded() {
    let now = policy::now_millis();
    let mut ctx = context(now);
    assert_eq!(
        authorized(&ctx, Action::Read, now)
            .unwrap()
            .organization_ceiling()
            .unwrap(),
        [uuid::Uuid::parse_str(ORG).unwrap()]
    );
    ctx["organizationScopeIds"] = json!(null);
    assert!(authorized(&ctx, Action::Read, now)
        .unwrap()
        .organization_ceiling()
        .is_none());
    ctx["organizationScopeIds"] = json!([ORG, CHILD, ORG.to_uppercase()]);
    assert_eq!(
        authorized(&ctx, Action::Read, now)
            .unwrap()
            .organization_ceiling()
            .unwrap()
            .len(),
        2
    );
    for ceiling in [
        json!([]),
        json!([CHILD]),
        json!(ORG),
        json!([ORG, null]),
        json!(vec![ORG; 10_001]),
    ] {
        ctx["organizationScopeIds"] = ceiling;
        assert_eq!(
            authorized(&ctx, Action::Read, now).err().unwrap().code(),
            "INVALID_CONTEXT"
        );
    }
}

#[test]
fn permissions_and_single_identity_bound_grants_are_independent() {
    let now = policy::now_millis();
    let mut ctx = context(now);
    ctx["permissions"] = json!(["ticket:export"]);
    assert_eq!(
        authorized(&ctx, Action::Read, now).err().unwrap().code(),
        "OPERATION_DENIED"
    );
    for (pointer, value) in [
        ("/dataGrants/0/tenantId", json!(CHILD)),
        ("/dataGrants/0/subjectUserId", json!(CHILD)),
        ("/dataGrants/0/scope", json!("ALL")),
        ("/dataGrants/0/organizationIds", json!(null)),
        ("/dataGrants/0/organizationIds", json!(vec![ORG; 10_001])),
    ] {
        let mut ctx = context(now);
        *ctx.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            authorized(&ctx, Action::Read, now).err().unwrap().code(),
            "DATA_DENIED",
            "{pointer}"
        );
    }
    let mut ctx = context(now);
    ctx["dataGrants"] = json!([]);
    assert_eq!(
        authorized(&ctx, Action::Read, now).err().unwrap().code(),
        "DATA_DENIED"
    );
    let mut ctx = context(now);
    let duplicate = ctx["dataGrants"][0].clone();
    ctx["dataGrants"].as_array_mut().unwrap().push(duplicate);
    assert_eq!(
        authorized(&ctx, Action::Read, now).err().unwrap().code(),
        "DATA_DENIED"
    );
}

#[test]
fn four_scopes_validate_organization_membership_and_parameterize_all_identities() {
    let now = policy::now_millis();
    for (scope, ids, expected) in [
        ("SELF", json!([]), Scope::SelfOnly),
        ("SELF", json!([ORG, CHILD]), Scope::SelfOnly),
        ("ORG", json!([ORG, ORG]), Scope::Org),
        ("ORG_SUBTREE", json!([ORG, CHILD]), Scope::OrgSubtree),
        ("TENANT", json!([]), Scope::Tenant),
    ] {
        let mut ctx = context(now);
        ctx["dataGrants"][0]["scope"] = json!(scope);
        ctx["dataGrants"][0]["organizationIds"] = ids;
        let auth = authorized(&ctx, Action::Read, now).unwrap();
        assert_eq!(auth.scope(), expected);
        auth.require_action(Action::Read).unwrap();
        auth.assert_fresh().unwrap();
        assert_eq!(
            auth.require_action(Action::Export).unwrap_err().code(),
            "OPERATION_DENIED"
        );
        let mut query = QueryBuilder::new("SELECT id FROM work_orders WHERE ");
        auth.append_scope(&mut query);
        let sql = query.sql();
        assert!(
            sql.contains("tenant_id=$1 AND deleted_at IS NULL AND organization_id=ANY($2::uuid[])")
        );
        assert!(!sql.contains(TENANT));
        assert!(!sql.contains(USER));
        assert!(!sql.contains(ORG));
        if expected == Scope::SelfOnly {
            assert!(sql.contains("owner_user_id=$3"));
        }
    }
    for (scope, ids) in [
        ("SELF", json!([CHILD])),
        ("ORG", json!([])),
        ("ORG", json!([ORG, CHILD])),
        ("ORG_SUBTREE", json!([CHILD])),
        ("TENANT", json!([ORG])),
    ] {
        let mut ctx = context(now);
        ctx["dataGrants"][0]["scope"] = json!(scope);
        ctx["dataGrants"][0]["organizationIds"] = ids;
        assert_eq!(
            authorized(&ctx, Action::Read, now).err().unwrap().code(),
            "DATA_DENIED"
        );
    }
    let past = now - 40_000;
    assert_eq!(
        authorized(&context(past), Action::Read, past)
            .unwrap()
            .assert_fresh()
            .unwrap_err()
            .code(),
        "INVALID_CONTEXT"
    );
}

struct Fixture {
    mode: AtomicU8,
    calls: AtomicUsize,
    headers: Mutex<Vec<(String, String)>>,
}

async fn fixture_response(
    State(state): State<Arc<Fixture>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> Response {
    state.calls.fetch_add(1, Ordering::SeqCst);
    state.headers.lock().unwrap().push((
        headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned(),
        headers
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned(),
    ));
    if uri.path().ends_with("/authorize") {
        let pairs = url::form_urlencoded::parse(&body)
            .into_owned()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs["resource"], "ticket");
        assert_eq!(pairs["action"], "read");
    } else if uri.path() == "/oauth/introspect" {
        assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
        let pairs = url::form_urlencoded::parse(&body).collect::<Vec<_>>();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "token");
        assert!(!pairs[0].1.is_empty());
    } else {
        assert!(body.is_empty());
    }
    let mode = state.mode.load(Ordering::SeqCst);
    if mode == 10 {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"code":"OPERATION_DENIED"})),
        )
            .into_response();
    }
    if mode == 2 {
        return (StatusCode::FOUND, [("location", "/unexpected")], "").into_response();
    }
    if mode == 3 {
        return ([("content-type", "text/html")], "secret upstream detail").into_response();
    }
    if mode == 4 {
        return ([("content-type", "application/json")], "{broken secret").into_response();
    }
    if mode == 5 {
        return (
            [("content-type", "application/json")],
            " ".repeat(256 * 1024 + 1),
        )
            .into_response();
    }
    if mode == 6 {
        return (StatusCode::UNAUTHORIZED, "upstream secret").into_response();
    }
    if mode == 7 {
        return (StatusCode::FORBIDDEN, "upstream secret").into_response();
    }
    if mode == 8 {
        return (StatusCode::INTERNAL_SERVER_ERROR, "upstream secret").into_response();
    }
    if mode == 9 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let now = policy::now_millis();
    if uri.path().ends_with("/authorize") {
        Json(json!({"contractVersion":"1.0","introspection":intro(now),"context":context(now)}))
            .into_response()
    } else if uri.path() == "/oauth/introspect" {
        let mut value = intro(now);
        if mode == 1 {
            value["active"] = json!(false);
        }
        Json(value).into_response()
    } else {
        Json(context(now)).into_response()
    }
}

async fn fixture(
    timeout: &str,
    max: &str,
) -> (Authorizer, Arc<Fixture>, tokio::task::JoinHandle<()>) {
    fixture_mode(timeout, max, "legacy").await
}

async fn fixture_mode(
    timeout: &str,
    max: &str,
    authorization_mode: &str,
) -> (Authorizer, Arc<Fixture>, tokio::task::JoinHandle<()>) {
    let state = Arc::new(Fixture {
        mode: AtomicU8::new(0),
        calls: AtomicUsize::new(0),
        headers: Mutex::new(Vec::new()),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/oauth/introspect", post(fixture_response))
        .route("/api/apps/work-orders/authorize", post(fixture_response))
        .route("/api/apps/work-orders/context", get(fixture_response))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let config = Config::from_pairs([
        (
            "WORK_ORDERS_DATABASE_URL",
            "postgresql://worker:secret@127.0.0.1/orders",
        ),
        ("PLATFORM_API_ORIGIN", &origin),
        ("PLATFORM_CLIENT_ID", "client"),
        ("PLATFORM_AUTHORIZATION_MODE", authorization_mode),
        ("PLATFORM_CLIENT_SECRET", "secret"),
        ("PLATFORM_AUTH_TIMEOUT_MS", timeout),
        ("PLATFORM_AUTH_MAX_CONCURRENT", max),
    ])
    .unwrap();
    (Authorizer::new(&config).unwrap(), state, server)
}

#[tokio::test]
async fn combined_authorization_keeps_independent_policy_and_never_caches() {
    let (auth, state, server) = fixture_mode("3000", "2", "combined").await;
    for _ in 0..2 {
        auth.authorize("app-token", Action::Read, REQUEST_ID)
            .await
            .unwrap();
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
    for (authorization, request_id) in state.headers.lock().unwrap().iter() {
        assert!(authorization.starts_with("Basic "));
        assert_eq!(request_id, REQUEST_ID);
    }
    state.mode.store(7, Ordering::SeqCst);
    assert!(auth
        .authorize("app-token", Action::Read, REQUEST_ID)
        .await
        .is_err());
    assert_eq!(state.calls.load(Ordering::SeqCst), 3);
    state.mode.store(10, Ordering::SeqCst);
    assert_eq!(
        auth.authorize("app-token", Action::Read, REQUEST_ID)
            .await
            .err()
            .unwrap()
            .code(),
        "OPERATION_DENIED"
    );
    server.abort();
}

#[tokio::test]
async fn fresh_basic_introspection_then_bearer_context_are_never_positive_cached() {
    let (auth, state, server) = fixture("3000", "2").await;
    auth.authorize("app-token", Action::Read, REQUEST_ID)
        .await
        .unwrap();
    let seen = state.headers.lock().unwrap().clone();
    assert_eq!(
        seen,
        [
            ("Basic Y2xpZW50OnNlY3JldA==".into(), REQUEST_ID.into()),
            ("Bearer app-token".into(), REQUEST_ID.into())
        ]
    );
    state.mode.store(1, Ordering::SeqCst);
    assert_eq!(
        auth.authorize("app-token", Action::Read, REQUEST_ID)
            .await
            .err()
            .unwrap()
            .code(),
        "INVALID_APP_TOKEN"
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), 3);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn upstream_status_body_redirect_json_and_capacity_fail_closed() {
    let (auth, state, server) = fixture("3000", "2").await;
    for (mode, code) in [
        (2, "PLATFORM_UNAVAILABLE"),
        (3, "PLATFORM_UNAVAILABLE"),
        (4, "PLATFORM_UNAVAILABLE"),
        (5, "PLATFORM_UNAVAILABLE"),
        (6, "INVALID_APP_TOKEN"),
        (7, "DATA_DENIED"),
        (8, "PLATFORM_UNAVAILABLE"),
    ] {
        state.mode.store(mode, Ordering::SeqCst);
        let error = auth
            .authorize("app-token", Action::Read, REQUEST_ID)
            .await
            .err()
            .unwrap();
        assert_eq!(error.code(), code);
        assert!(!format!("{error:?} {error}").contains("secret"));
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 7);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn shared_authorization_budget_and_concurrency_bound_do_not_queue_requests() {
    let (auth, state, server) = fixture("100", "1").await;
    let permit = auth.concurrent.clone().acquire_owned().await.unwrap();
    assert_eq!(
        auth.authorize("token", Action::Read, REQUEST_ID)
            .await
            .err()
            .unwrap()
            .code(),
        "PLATFORM_UNAVAILABLE"
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    drop(permit);
    state.mode.store(9, Ordering::SeqCst);
    let started = std::time::Instant::now();
    assert_eq!(
        auth.authorize("token", Action::Read, REQUEST_ID)
            .await
            .err()
            .unwrap()
            .code(),
        "PLATFORM_UNAVAILABLE"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(auth.concurrent.available_permits(), 1);
    server.abort();
    let _ = server.await;
}

#[test]
fn protected_application_requires_platform_configuration() {
    let config = Config::from_pairs([(
        "WORK_ORDERS_DATABASE_URL",
        "postgresql://worker:secret@127.0.0.1/orders",
    )])
    .unwrap();
    let error = Authorizer::new(&config).err().unwrap();
    assert_eq!(
        error.to_string(),
        "platform authorization configuration is required"
    );
    assert!(!format!("{error:?}").contains("secret"));
}
