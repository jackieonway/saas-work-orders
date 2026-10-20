//! Explicit owned PostgreSQL + controlled platform HTTP fixture member contracts.
//! Production platform and business databases are never used by this fixture.
use std::{
    collections::{BTreeMap, HashMap},
    future::{Future, IntoFuture},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use chrono::{SecondsFormat, Utc};
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, Executor, PgPool};
use tokio::{
    net::TcpListener,
    sync::{Notify, Semaphore},
    task::JoinHandle,
};
use uuid::Uuid;
use work_orders_api::{application, connect_database, initialize_database, Config};

const PREFIX: &str = "/work-orders-api";
const SECRET: &str = "phase19-fixture-secret";
const BASIC: &str = "Basic aW50ZWdyYXRpb246cGhhc2UxOS1maXh0dXJlLXNlY3JldA==";
const RECEIPTS: &str = "public.work_order_create_idempotency";

#[derive(Clone, Copy)]
struct Identity {
    tenant: Uuid,
    org: Uuid,
    user: Uuid,
}

#[derive(Clone)]
struct Fixture {
    pool: PgPool,
    database_url: String,
    identity: Identity,
    pools: Arc<Mutex<Vec<PgPool>>>,
}

async fn owned<F, Fut>(case: F)
where
    F: FnOnce(Fixture) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let raw = std::env::var("TEST_DATABASE_URL").expect("owned TEST_DATABASE_URL required");
    let owner = std::env::var("CI_TEST_POSTGRES_OWNER").expect("owned CI owner required");
    assert!(
        owner.starts_with("ci-") && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    );
    let mut url = url::Url::parse(&raw).expect("valid owned URL");
    assert_eq!(url.host_str(), Some(format!("{owner}-postgres").as_str()));
    assert_eq!(url.path(), "/postgres");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&raw)
        .await
        .expect("owned admin");
    let name = format!("work_orders_phase19_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&admin)
        .await
        .expect("create owned database");
    // No cleanup path can receive this name before its CREATE succeeded.
    url.set_path(&format!("/{name}"));
    let pool = PgPoolOptions::new().max_connections(5)
        .after_connect(|connection, _| Box::pin(async move {
            connection.execute("SET statement_timeout='20s'; SET lock_timeout='5s'; SET search_path=pg_catalog,public,pg_temp").await?;
            Ok(())
        })).connect(url.as_str()).await;
    let pool = match pool {
        Ok(pool) => pool,
        Err(_) => {
            let removed = sqlx::query(&format!("DROP DATABASE \"{name}\" WITH (FORCE)"))
                .execute(&admin)
                .await
                .is_ok();
            admin.close().await;
            println!("WORK_ORDERS_MEMBER_FIXTURE_SETUP_CLEANUP database_removed={removed} pool_closed=true");
            assert!(removed, "owned setup cleanup failed");
            panic!("owned observer pool setup failed");
        }
    };
    let pools = Arc::new(Mutex::new(vec![pool.clone()]));
    let fixture = Fixture {
        pool,
        database_url: url.to_string(),
        identity: Identity {
            tenant: Uuid::new_v4(),
            org: Uuid::new_v4(),
            user: Uuid::new_v4(),
        },
        pools: pools.clone(),
    };
    let result = tokio::spawn(async move {
        initialize_database(&fixture.pool)
            .await
            .expect("current business migrations");
        case(fixture).await;
    })
    .await;
    let tracked = pools.lock().expect("tracked pools").clone();
    for pool in tracked {
        pool.close().await;
    }
    let removed = sqlx::query(&format!("DROP DATABASE \"{name}\" WITH (FORCE)"))
        .execute(&admin)
        .await
        .is_ok();
    admin.close().await;
    println!(
        "WORK_ORDERS_MEMBER_FIXTURE_CLEANUP assertions_ok={} database_removed={} pool_closed=true",
        result.is_ok(),
        removed
    );
    assert!(removed, "owned database cleanup failed");
    if let Err(error) = result {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("owned member task cancelled");
    }
}

#[derive(Clone)]
struct Gate {
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
}

impl Gate {
    fn new() -> Self {
        Self {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Semaphore::new(0)),
        }
    }
    async fn observed(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.notified())
            .await
            .expect("platform context gate reached");
    }
}

#[derive(Clone)]
struct Scenario {
    identity: Identity,
    permissions: Vec<String>,
    grants: Vec<String>,
    scope: &'static str,
    organization_ids: Vec<Uuid>,
    ceiling: Value,
    expires_at: Option<chrono::DateTime<Utc>>,
    organization_name: String,
    display_name: String,
    gate: Option<Gate>,
    member_mode: MemberMode,
}

impl Scenario {
    fn allowed(identity: Identity) -> Self {
        let actions = ["read", "export", "create", "update", "delete"];
        Self {
            identity,
            permissions: actions.iter().map(|a| format!("ticket:{a}")).collect(),
            grants: actions.iter().map(|a| (*a).to_owned()).collect(),
            scope: "TENANT",
            organization_ids: vec![],
            ceiling: Value::Null,
            expires_at: None,
            organization_name: "写入组织".into(),
            display_name: "  写入用户  ".into(),
            gate: None,
            member_mode: MemberMode::Normal,
        }
    }

    fn introspection(&self) -> Value {
        json!({"active":true,"aud":"work-orders","sub":self.identity.user.to_string(),
            "tenant_id":self.identity.tenant.to_string(),"organization_id":self.identity.org.to_string(),
            "exp":Utc::now().timestamp()+600,"token_type":"Bearer"})
    }

    fn context(&self) -> Value {
        let expires = self
            .expires_at
            .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(28));
        json!({"contractVersion":"1.0","appCode":"work-orders","contextVersion":"b".repeat(64),
            "expiresAt":expires.to_rfc3339_opts(SecondsFormat::Millis,true),
            "user":{"id":self.identity.user.to_string(),"username":"writer","displayName":self.display_name},
            "tenant":{"id":self.identity.tenant.to_string()},
            "organization":{"id":self.identity.org.to_string(),"name":self.organization_name},
            "application":{"id":"33333333-3333-4333-8333-333333333333"},"organizationScopeIds":self.ceiling,
            "permissions":self.permissions,"dataGrants":self.grants.iter().map(|action|json!({
                "resource":"ticket","action":action,"tenantId":self.identity.tenant.to_string(),
                "subjectUserId":self.identity.user.to_string(),"scope":self.scope,
                "organizationIds":self.organization_ids.iter().map(Uuid::to_string).collect::<Vec<_>>()
            })).collect::<Vec<_>>()})
    }
}

#[derive(Default)]
struct PlatformState {
    scenarios: HashMap<String, Scenario>,
    introspections: usize,
    contexts: usize,
    members: usize,
    member_records: HashMap<(Uuid, Uuid, Uuid), MemberRecord>,
    member_calls: Vec<MemberCall>,
    redirect_hits: usize,
}

#[derive(Clone, Default)]
enum MemberMode {
    #[default]
    Normal,
    Status(u16),
    Patch(Value),
    Malformed,
    Html,
    Oversize,
    Redirect,
}

#[derive(Clone)]
struct MemberRecord {
    identity: Identity,
    display_name: String,
    enabled: bool,
    direct: bool,
}

#[derive(Clone)]
struct MemberCall {
    token: String,
    action: String,
    organization: Uuid,
    user: Option<Uuid>,
    query: BTreeMap<String, String>,
    request_id: String,
    started: Instant,
}

type Shared = Arc<Mutex<PlatformState>>;

async fn introspect(State(shared): State<Shared>, request: Request) -> Response {
    assert_eq!(request.headers().get("authorization").unwrap(), BASIC);
    assert_eq!(
        request.headers().get("content-type").unwrap(),
        "application/x-www-form-urlencoded"
    );
    let bytes = to_bytes(request.into_body(), 16384)
        .await
        .expect("bounded fixture introspection");
    let fields: Vec<_> = url::form_urlencoded::parse(&bytes).into_owned().collect();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].0, "token");
    let scenario = {
        let mut state = shared.lock().expect("platform state");
        state.introspections += 1;
        state.scenarios.get(&fields[0].1).cloned()
    };
    match scenario {
        Some(scenario) => Json(scenario.introspection()).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn context(State(shared): State<Shared>, request: Request) -> Response {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let scenario = {
        let mut state = shared.lock().expect("platform state");
        state.contexts += 1;
        state.scenarios.get(token).cloned()
    };
    let Some(scenario) = scenario else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    Json(scenario.context()).into_response()
}

async fn members(State(shared): State<Shared>, request: Request) -> Response {
    assert_eq!(request.method(), Method::GET);
    let token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .expect("member request uses application bearer")
        .to_owned();
    let pairs: Vec<_> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    let query: BTreeMap<_, _> = pairs.iter().cloned().collect();
    assert_eq!(
        pairs.len(),
        query.len(),
        "outgoing member query has no duplicates"
    );
    assert_eq!(query.get("resource").map(String::as_str), Some("ticket"));
    let action = query.get("action").expect("bound member action").clone();
    assert!(matches!(action.as_str(), "create" | "update"));
    let organization = Uuid::parse_str(query.get("organizationId").unwrap()).unwrap();
    let user = request
        .uri()
        .path()
        .strip_prefix("/api/apps/work-orders/members/")
        .map(|id| Uuid::parse_str(id).expect("canonical member path"));
    assert_eq!(query.len(), if user.is_some() { 3 } else { 6 });
    let request_id = request
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(Uuid::parse_str(&request_id).is_ok());
    let scenario = {
        let mut state = shared.lock().unwrap();
        state.members += 1;
        state.member_calls.push(MemberCall {
            token: token.clone(),
            action,
            organization,
            user,
            query: query.clone(),
            request_id,
            started: Instant::now(),
        });
        state.scenarios.get(&token).cloned()
    };
    let Some(scenario) = scenario else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if let Some(gate) = &scenario.gate {
        gate.entered.notify_one();
        let permit = gate
            .release
            .acquire()
            .await
            .expect("controlled member release");
        permit.forget();
    }
    let mut body = {
        let state = shared.lock().unwrap();
        if let Some(user) = user {
            let member = state
                .member_records
                .get(&(scenario.identity.tenant, organization, user))
                .filter(|member| member.enabled && member.direct);
            let Some(member) = member else {
                return StatusCode::NOT_FOUND.into_response();
            };
            json!({"id":member.identity.user.to_string(),"organizationId":member.identity.org.to_string(),"displayName":member.display_name})
        } else {
            let page: usize = query.get("page").unwrap().parse().unwrap();
            let size: usize = query.get("size").unwrap().parse().unwrap();
            let q = query.get("q").unwrap().to_lowercase();
            let mut selected: Vec<_> = state
                .member_records
                .values()
                .filter(|member| {
                    member.identity.tenant == scenario.identity.tenant
                        && member.identity.org == organization
                        && member.enabled
                        && member.direct
                        && member.display_name.to_lowercase().contains(&q)
                })
                .cloned()
                .collect();
            selected.sort_by_key(|member| member.identity.user);
            let total = selected.len();
            let records:Vec<_>=selected.into_iter().skip((page-1)*size).take(size).map(|member|
                json!({"id":member.identity.user.to_string(),"organizationId":member.identity.org.to_string(),"displayName":member.display_name})).collect();
            json!({"page":page,"size":size,"total":total,"records":records})
        }
    };
    match scenario.member_mode {
        MemberMode::Normal => Json(body).into_response(),
        MemberMode::Status(status) => StatusCode::from_u16(status).unwrap().into_response(),
        MemberMode::Patch(patch) => {
            for (key, value) in patch.as_object().expect("member response patch") {
                body[key] = value.clone();
            }
            Json(body).into_response()
        }
        MemberMode::Malformed => (
            [("content-type", "application/json")],
            format!("{{bad member {SECRET}"),
        )
            .into_response(),
        MemberMode::Html => (
            [("content-type", "text/html")],
            format!("<html>{SECRET}</html>"),
        )
            .into_response(),
        MemberMode::Oversize => {
            body["padding"] = json!("x".repeat(300_000));
            Json(body).into_response()
        }
        MemberMode::Redirect => (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "/redirect-trap"), ("connection", "close")],
            "",
        )
            .into_response(),
    }
}

async fn redirect_trap(State(shared): State<Shared>) -> Response {
    shared.lock().unwrap().redirect_hits += 1;
    Json(json!({"id":"33333333-3333-4333-8333-333333333333","organizationId":"33333333-3333-4333-8333-333333333333","displayName":"must not be reached"})).into_response()
}

struct Harness {
    client: reqwest::Client,
    origin: String,
    pool: PgPool,
    state: Shared,
    tasks: Vec<JoinHandle<std::io::Result<()>>>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("JSON response")
    }
    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).expect("UTF-8 response")
    }
    fn error(&self, status: u16, code: &str) {
        assert_eq!(self.status, status, "{}", self.text());
        assert_eq!(self.json()["code"], code);
        assert_eq!(
            self.headers.get("access-control-allow-origin").unwrap(),
            "null"
        );
        assert!(!self
            .headers
            .contains_key("access-control-allow-credentials"));
        assert!(
            Uuid::parse_str(self.headers.get("x-request-id").unwrap().to_str().unwrap()).is_ok()
        );
        assert!(!self.text().contains(SECRET));
        assert!(!self.text().contains(BASIC));
    }
}

impl Harness {
    async fn new(f: &Fixture) -> Self {
        Self::configured(f, &[]).await
    }

    async fn configured(f: &Fixture, overrides: &[(&str, &str)]) -> Self {
        let state = Shared::default();
        let platform = Router::new()
            .route("/oauth/introspect", post(introspect))
            .route("/api/apps/work-orders/context", get(context))
            .route("/api/apps/work-orders/members", get(members))
            .route("/api/apps/work-orders/members/{id}", get(members))
            .route("/redirect-trap", any(redirect_trap))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("platform fixture listener");
        let platform_origin = format!("http://{}", listener.local_addr().unwrap());
        let platform_task = tokio::spawn(axum::serve(listener, platform).into_future());
        let mut pairs: BTreeMap<String, String> = [
            ("WORK_ORDERS_DATABASE_URL", f.database_url.as_str()),
            ("PLATFORM_API_ORIGIN", platform_origin.as_str()),
            ("PLATFORM_CLIENT_ID", "integration"),
            ("PLATFORM_CLIENT_SECRET", SECRET),
            ("PLATFORM_AUTHORIZATION_MODE", "legacy"),
            ("PLATFORM_AUTH_TIMEOUT_MS", "3000"),
            ("WORK_ORDERS_DB_MAX_CONNECTIONS", "2"),
            ("WORK_ORDERS_DB_ACQUIRE_TIMEOUT_MS", "1000"),
            ("WORK_ORDERS_DB_STATEMENT_TIMEOUT_MS", "5000"),
            ("WORK_ORDERS_DB_LOCK_TIMEOUT_MS", "500"),
            ("WORK_ORDERS_ALLOWED_ORIGINS", "null"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        for (key, value) in overrides {
            pairs.insert((*key).to_owned(), (*value).to_owned());
        }
        let config = Config::from_pairs(pairs).expect("write fixture config");
        let pool = connect_database(&config)
            .await
            .expect("bounded application pool");
        f.pools.lock().unwrap().push(pool.clone());
        let app = application(&config, pool.clone()).expect("write application");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("business fixture listener");
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let business_task = tokio::spawn(axum::serve(listener, app).into_future());
        let h = Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(25))
                .build()
                .unwrap(),
            origin,
            pool,
            state,
            tasks: vec![platform_task, business_task],
        };
        h.put("writer", Scenario::allowed(f.identity));
        h
    }

    fn put(&self, token: &str, scenario: Scenario) {
        self.state
            .lock()
            .unwrap()
            .scenarios
            .insert(token.to_owned(), scenario);
    }

    async fn raw(
        &self,
        method: Method,
        token: &str,
        path: &str,
        key: Option<&str>,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Reply {
        let mut request = self
            .client
            .request(method, format!("{}{PREFIX}{path}", self.origin))
            .header("origin", "null")
            .bearer_auth(token)
            .body(body);
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        let response = request.send().await.expect("business write HTTP response");
        Reply {
            status: response.status().as_u16(),
            headers: response.headers().clone(),
            body: response.bytes().await.unwrap().to_vec(),
        }
    }

    async fn request(
        &self,
        method: Method,
        token: &str,
        path: &str,
        key: Option<&str>,
        body: Value,
    ) -> Reply {
        self.raw(
            method,
            token,
            path,
            key,
            serde_json::to_vec(&body).unwrap(),
            Some("application/json"),
        )
        .await
    }

    async fn create(&self, key: &str, title: &str) -> Reply {
        self.request(
            Method::POST,
            "writer",
            "/tickets",
            Some(key),
            json!({"title":title}),
        )
        .await
    }

    fn no_directory(&self) {
        assert_eq!(
            self.state.lock().unwrap().members,
            0,
            "no Phase19 directory call"
        );
    }

    fn add_member(&self, identity: Identity, name: &str) {
        self.state.lock().unwrap().member_records.insert(
            (identity.tenant, identity.org, identity.user),
            MemberRecord {
                identity,
                display_name: name.to_owned(),
                enabled: true,
                direct: true,
            },
        );
    }

    fn member_calls(&self) -> Vec<MemberCall> {
        self.state.lock().unwrap().member_calls.clone()
    }

    async fn get(&self, token: &str, path: &str) -> Reply {
        self.raw(Method::GET, token, path, None, Vec::new(), None)
            .await
    }
}

async fn counts(f: &Fixture) -> (i64, i64) {
    let tickets = sqlx::query_scalar("SELECT count(*) FROM public.work_orders")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let receipts = sqlx::query_scalar(&format!("SELECT count(*) FROM {RECEIPTS}"))
        .fetch_one(&f.pool)
        .await
        .unwrap();
    (tickets, receipts)
}

async fn counter(f: &Fixture, tenant: Uuid) -> Option<i64> {
    sqlx::query_scalar("SELECT next_number FROM public.work_order_counters WHERE tenant_id=$1")
        .bind(tenant)
        .fetch_optional(&f.pool)
        .await
        .unwrap()
}

async fn row(f: &Fixture, id: &str) -> Value {
    let raw: String =
        sqlx::query_scalar("SELECT row_to_json(t)::text FROM public.work_orders t WHERE id=$1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    serde_json::from_str(&raw).unwrap()
}

async fn wait_for_lock(f: &Fixture, query_fragment: &str) {
    let fragment = format!("%{query_fragment}%");
    let mut poll = tokio::time::interval(Duration::from_millis(10));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tokio::time::timeout(Duration::from_secs(5),async {
        loop {
            poll.tick().await;
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND state='active' AND wait_event_type='Lock' AND query LIKE $1 AND pid<>pg_backend_pid())")
                .bind(&fragment).fetch_one(&f.pool).await.expect("controlled lock observer");
            if waiting {break;}
        }
    }).await.expect("application observed waiting on owned database lock");
}

fn jids(ids: &[Uuid]) -> Value {
    json!(ids.iter().map(Uuid::to_string).collect::<Vec<_>>())
}

fn query_params(pairs: &[(&str, &str)]) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        query.append_pair(key, value);
    }
    query.finish()
}

fn selected(identity: Identity, user: Uuid, organization: Uuid) -> Identity {
    Identity {
        user,
        org: organization,
        ..identity
    }
}

fn member_row(id: Uuid, organization: Uuid, name: &str) -> Value {
    json!({"id":id.to_string(),"organizationId":organization.to_string(),"displayName":name})
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn directory_search_pagination_contract() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        for index in 0..5 {
            let name = if index == 4 {
                "Literal 100%_\\ member".to_owned()
            } else {
                format!("Alpha 中文 {index}")
            };
            h.add_member(selected(f.identity, Uuid::new_v4(), f.identity.org), &name);
        }
        let first = h.get("writer", "/assignees?action=create").await;
        assert_eq!(first.status, 200, "{}", first.text());
        assert_eq!(first.json()["page"], 1);
        assert_eq!(first.json()["size"], 20);
        assert_eq!(first.json()["total"], 5);
        let all = first.json();
        let records = all["records"].as_array().unwrap();
        assert_eq!(records.len(), 5);
        let mut sorted: Vec<_> = records
            .iter()
            .map(|record| record["userId"].as_str().unwrap().to_owned())
            .collect();
        let actual = sorted.clone();
        sorted.sort();
        assert_eq!(actual, sorted);
        for record in records {
            let keys: std::collections::BTreeSet<_> = record
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                keys,
                ["userId", "organizationId", "displayName"]
                    .into_iter()
                    .collect()
            );
            assert_eq!(record["organizationId"], f.identity.org.to_string());
        }
        let query = query_params(&[
            ("action", "update"),
            ("q", "  Alpha  "),
            ("page", "2"),
            ("size", "2"),
        ]);
        let page = h.get("writer", &format!("/assignees?{query}")).await;
        assert_eq!(page.status, 200);
        assert_eq!(page.json()["total"], 4);
        assert_eq!(page.json()["records"].as_array().unwrap().len(), 2);
        let literal = query_params(&[("action", "create"), ("q", "%_\\")]);
        let literal = h.get("writer", &format!("/assignees?{literal}")).await;
        assert_eq!(literal.status, 200);
        assert_eq!(literal.json()["total"], 1);
        let empty = h
            .get("writer", "/assignees?action=create&page=1000000&size=50")
            .await;
        assert_eq!(empty.status, 200);
        assert_eq!(empty.json()["total"], 5);
        assert_eq!(empty.json()["records"], json!([]));
        for call in h.member_calls() {
            assert_eq!(call.token, "writer");
            assert!(matches!(call.action.as_str(), "create" | "update"));
            assert_eq!(call.organization, f.identity.org);
            assert!(call.user.is_none());
            assert_eq!(call.query["resource"], "ticket");
            assert!(Uuid::parse_str(&call.request_id).is_ok());
            if call.action == "update" {
                assert_eq!(call.query["q"], "Alpha");
            }
        }
        assert_eq!(counts(&f).await, (0, 0));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn directory_input_and_upstream_shape_rejection() {
    owned(|f|async move {
        let h=Harness::new(&f).await;let member=Uuid::new_v4();h.add_member(selected(f.identity,member,f.identity.org),"Member");
        for query in ["","action=read","action=delete","action=create&size=51","action=create&size=0","action=create&page=0",
            "action=create&page=1000001","action=create&page=18446744073709551616","action=create&organizationId=invalid",
            "action=create&tenantId=invalid","action=create&action=update","action=create&q=a&q=b"] {
            h.get("writer",&format!("/assignees?{query}")).await.error(400,"VALIDATION_ERROR");
        }
        let too_long=query_params(&[("action","create"),("q",&"😀".repeat(101))]);
        h.get("writer",&format!("/assignees?{too_long}")).await.error(400,"VALIDATION_ERROR");h.no_directory();
        let valid=member_row(member,f.identity.org,"Member");
        for patch in [json!({"page":2}),json!({"size":2}),json!({"total":-1}),json!({"total":1.5}),json!({"total":"1"}),json!({"records":null}),
            json!({"records":[valid.clone(),valid.clone()]}),json!({"records":[member_row(Uuid::new_v4(),Uuid::new_v4(),"other org")]}),
            json!({"records":[{"id":"invalid","organizationId":f.identity.org.to_string(),"displayName":"Member"}]}),
            json!({"records":[member_row(member,f.identity.org," ")]}),json!({"records":[member_row(member,f.identity.org,&"界".repeat(101))]})] {
            let mut scenario=Scenario::allowed(f.identity);scenario.member_mode=MemberMode::Patch(patch);h.put("malformed",scenario);
            h.get("malformed","/assignees?action=create&size=1").await.error(503,"PLATFORM_UNAVAILABLE");
        }
        assert_eq!(counts(&f).await,(0,0));
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn directory_authorization_and_scope_matrix() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let child = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let parent = Uuid::new_v4();
        for organization in [f.identity.org, child, peer, parent] {
            h.add_member(selected(f.identity, Uuid::new_v4(), organization), "Member");
        }
        let cases = [
            (
                "SELF",
                vec![],
                Value::Null,
                vec![f.identity.org],
                vec![child, peer, parent],
            ),
            (
                "ORG",
                vec![f.identity.org],
                Value::Null,
                vec![f.identity.org],
                vec![child, peer, parent],
            ),
            (
                "ORG_SUBTREE",
                vec![f.identity.org, child],
                Value::Null,
                vec![f.identity.org, child],
                vec![peer, parent],
            ),
            (
                "ORG_SUBTREE",
                vec![f.identity.org, child],
                jids(&[f.identity.org]),
                vec![f.identity.org],
                vec![child, peer, parent],
            ),
            (
                "TENANT",
                vec![],
                jids(&[f.identity.org, child]),
                vec![f.identity.org, child],
                vec![peer, parent],
            ),
            (
                "TENANT",
                vec![],
                Value::Null,
                vec![f.identity.org, child, peer, parent],
                vec![],
            ),
        ];
        for (scope, organizations, ceiling, allowed, denied) in cases {
            let mut scenario = Scenario::allowed(f.identity);
            scenario.scope = scope;
            scenario.organization_ids = organizations;
            scenario.ceiling = ceiling;
            h.put("scoped", scenario);
            for organization in allowed {
                let reply = h
                    .get(
                        "scoped",
                        &format!("/assignees?action=update&organizationId={organization}"),
                    )
                    .await;
                assert_eq!(reply.status, 200, "{scope}: {}", reply.text());
            }
            for organization in denied {
                let before = h.member_calls().len();
                h.get(
                    "scoped",
                    &format!("/assignees?action=update&organizationId={organization}"),
                )
                .await
                .error(403, "DATA_DENIED");
                assert_eq!(
                    h.member_calls().len(),
                    before,
                    "out-of-scope directory never reaches upstream"
                );
            }
        }
        for permission_missing in [true, false] {
            let mut scenario = Scenario::allowed(f.identity);
            if permission_missing {
                scenario.permissions.retain(|p| p != "ticket:create");
            } else {
                scenario.grants.retain(|a| a != "create");
            }
            h.put("denied", scenario);
            let before = h.member_calls().len();
            h.get("denied", "/assignees?action=create").await.error(
                403,
                if permission_missing {
                    "OPERATION_DENIED"
                } else {
                    "DATA_DENIED"
                },
            );
            assert_eq!(h.member_calls().len(), before);
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn assigned_create_and_idempotent_replay() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let member = selected(f.identity, Uuid::new_v4(), f.identity.org);
        h.add_member(member, "Alice 原始快照");
        let body = json!({"title":"assigned creation","assigneeUserId":member.user.to_string()});
        let first = h
            .request(
                Method::POST,
                "writer",
                "/tickets",
                Some("assigned-receipt"),
                body.clone(),
            )
            .await;
        assert_eq!(first.status, 201, "{}", first.text());
        assert_eq!(first.json()["assigneeUserId"], member.user.to_string());
        assert_eq!(first.json()["assigneeDisplayName"], "Alice 原始快照");
        let calls = h.member_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].user, Some(member.user));
        assert_eq!(calls[0].action, "create");
        assert_eq!(calls[0].organization, f.identity.org);
        h.state
            .lock()
            .unwrap()
            .member_records
            .get_mut(&(member.tenant, member.org, member.user))
            .unwrap()
            .enabled = false;
        let replay = h
            .request(
                Method::POST,
                "writer",
                "/tickets",
                Some("assigned-receipt"),
                body.clone(),
            )
            .await;
        assert_eq!(replay.status, 201);
        assert_eq!(replay.body, first.body);
        assert_eq!(
            h.member_calls().len(),
            1,
            "committed receipt is recovery, not a new assignment"
        );
        let mut different = body.clone();
        different["title"] = json!("different request");
        h.request(
            Method::POST,
            "writer",
            "/tickets",
            Some("assigned-receipt"),
            different,
        )
        .await
        .error(409, "IDEMPOTENCY_CONFLICT");
        assert_eq!(h.member_calls().len(), 1);
        h.request(
            Method::POST,
            "writer",
            "/tickets",
            Some("new-after-removal"),
            body,
        )
        .await
        .error(404, "MEMBER_NOT_FOUND");
        assert_eq!(counts(&f).await, (1, 1));
        assert_eq!(counter(&f, f.identity.tenant).await, Some(1));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn assignment_update_clear_and_same_value_preserves_snapshot() {
    owned(|f|async move {
        let h=Harness::new(&f).await;let alice=selected(f.identity,Uuid::new_v4(),f.identity.org);let bob=selected(f.identity,Uuid::new_v4(),f.identity.org);
        h.add_member(alice,"Alice 历史名称");h.add_member(bob,"Bob 新名称");
        let first=h.request(Method::POST,"writer","/tickets",Some("assignment-update"),json!({"title":"before","assigneeUserId":alice.user.to_string()})).await;
        assert_eq!(first.status,201);let id=first.json()["id"].as_str().unwrap().to_owned();let path=format!("/tickets/{id}");
        {let mut state=h.state.lock().unwrap();let alice=state.member_records.get_mut(&(alice.tenant,alice.org,alice.user)).unwrap();alice.display_name="Alice 已更名".into();alice.enabled=false;}
        for (version,body) in [
            (1,json!({"version":1,"title":"same assignment","assigneeUserId":alice.user.to_string()})),
            (2,json!({"version":2,"title":"same uppercase UUID","assigneeUserId":alice.user.to_string().to_uppercase()})),
            (3,json!({"version":3,"title":"omitted assignment"})),
        ] {
            let reply=h.request(Method::PATCH,"writer",&path,None,body).await;assert_eq!(reply.status,200,"{}",reply.text());
            assert_eq!(reply.json()["version"],version+1);assert_eq!(reply.json()["assigneeDisplayName"],"Alice 历史名称");
            assert_eq!(h.member_calls().len(),1,"same assignment preserves its historic snapshot");
        }
        let changed=h.request(Method::PATCH,"writer",&path,None,json!({"version":4,"assigneeUserId":bob.user.to_string()})).await;
        assert_eq!(changed.status,200,"{}",changed.text());assert_eq!(changed.json()["assigneeUserId"],bob.user.to_string());assert_eq!(changed.json()["assigneeDisplayName"],"Bob 新名称");assert_eq!(h.member_calls().len(),2);
        let cleared=h.request(Method::PATCH,"writer",&path,None,json!({"version":5,"assigneeUserId":null})).await;
        assert_eq!(cleared.status,200);assert_eq!(cleared.json()["assigneeUserId"],Value::Null);assert_eq!(cleared.json()["assigneeDisplayName"],Value::Null);assert_eq!(h.member_calls().len(),2);
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn invalid_disabled_and_indirect_members_rejected() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let base = h.create("invalid-member-base", "before").await;
        assert_eq!(base.status, 201);
        let id = base.json()["id"].as_str().unwrap().to_owned();
        let path = format!("/tickets/{id}");
        let disabled = selected(f.identity, Uuid::new_v4(), f.identity.org);
        let indirect = selected(f.identity, Uuid::new_v4(), f.identity.org);
        let foreign_org = selected(f.identity, Uuid::new_v4(), Uuid::new_v4());
        let foreign_tenant = Identity {
            tenant: Uuid::new_v4(),
            user: Uuid::new_v4(),
            ..f.identity
        };
        for member in [disabled, indirect, foreign_org, foreign_tenant] {
            h.add_member(member, "Member");
        }
        {
            let mut state = h.state.lock().unwrap();
            state
                .member_records
                .get_mut(&(disabled.tenant, disabled.org, disabled.user))
                .unwrap()
                .enabled = false;
            state
                .member_records
                .get_mut(&(indirect.tenant, indirect.org, indirect.user))
                .unwrap()
                .direct = false;
        }
        for (i, user) in [
            disabled.user,
            indirect.user,
            foreign_org.user,
            foreign_tenant.user,
            Uuid::new_v4(),
        ]
        .into_iter()
        .enumerate()
        {
            h.request(
                Method::POST,
                "writer",
                "/tickets",
                Some(&format!("invalid-member-{i}")),
                json!({"title":"invalid member","assigneeUserId":user.to_string()}),
            )
            .await
            .error(404, "MEMBER_NOT_FOUND");
            h.request(
                Method::PATCH,
                "writer",
                &path,
                None,
                json!({"version":1,"assigneeUserId":user.to_string()}),
            )
            .await
            .error(404, "MEMBER_NOT_FOUND");
        }
        let before = h.member_calls().len();
        h.request(
            Method::POST,
            "writer",
            "/tickets",
            Some("malformed-member"),
            json!({"title":"invalid","assigneeUserId":"invalid"}),
        )
        .await
        .error(400, "VALIDATION_ERROR");
        h.request(
            Method::PATCH,
            "writer",
            &path,
            None,
            json!({"version":1,"assigneeUserId":"invalid"}),
        )
        .await
        .error(400, "VALIDATION_ERROR");
        assert_eq!(h.member_calls().len(), before);
        let valid = selected(f.identity, Uuid::new_v4(), f.identity.org);
        h.add_member(valid, "Valid");
        for patch in [
            json!({"id":Uuid::new_v4().to_string()}),
            json!({"organizationId":Uuid::new_v4().to_string()}),
            json!({"displayName":" "}),
            json!({"displayName":"界".repeat(101)}),
        ] {
            let mut scenario = Scenario::allowed(f.identity);
            scenario.member_mode = MemberMode::Patch(patch);
            h.put("misbound", scenario);
            h.request(
                Method::POST,
                "misbound",
                "/tickets",
                Some("misbound"),
                json!({"title":"misbound","assigneeUserId":valid.user.to_string()}),
            )
            .await
            .error(503, "PLATFORM_UNAVAILABLE");
        }
        assert_eq!(row(&f, &id).await["version"], 1);
        assert_eq!(row(&f, &id).await["assignee_user_id"], Value::Null);
        assert_eq!(counts(&f).await, (1, 1));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn member_transport_errors_are_bounded_and_secret_safe() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let member = selected(f.identity, Uuid::new_v4(), f.identity.org);
        h.add_member(member, "Member");
        let cases = [
            (MemberMode::Status(401), 401, "INVALID_APP_TOKEN"),
            (MemberMode::Status(403), 403, "DATA_DENIED"),
            (MemberMode::Status(404), 404, "MEMBER_NOT_FOUND"),
            (MemberMode::Status(500), 503, "PLATFORM_UNAVAILABLE"),
            (MemberMode::Malformed, 503, "PLATFORM_UNAVAILABLE"),
            (MemberMode::Html, 503, "PLATFORM_UNAVAILABLE"),
            (MemberMode::Oversize, 503, "PLATFORM_UNAVAILABLE"),
            (MemberMode::Redirect, 503, "PLATFORM_UNAVAILABLE"),
        ];
        for (index, (mode, status, code)) in cases.into_iter().enumerate() {
            let token = format!("transport-{index}");
            let mut scenario = Scenario::allowed(f.identity);
            scenario.member_mode = mode;
            h.put(&token, scenario);
            let started = Instant::now();
            h.request(
                Method::POST,
                &token,
                "/tickets",
                Some(&format!("transport-{index}")),
                json!({"title":"must fail closed","assigneeUserId":member.user.to_string()}),
            )
            .await
            .error(status, code);
            let list = h.get(&token, "/assignees?action=create").await;
            if status == 404 {
                list.error(503, "PLATFORM_UNAVAILABLE");
            } else {
                list.error(status, code);
            }
            assert!(started.elapsed() < Duration::from_secs(3));
        }
        assert_eq!(h.state.lock().unwrap().redirect_hits, 0);
        let timed = Arc::new(Harness::configured(&f, &[("PLATFORM_AUTH_TIMEOUT_MS", "150")]).await);
        timed.add_member(member, "Member");
        let gate = Gate::new();
        let mut scenario = Scenario::allowed(f.identity);
        scenario.gate = Some(gate.clone());
        timed.put("timed", scenario);
        let worker = timed.clone();
        let started = Instant::now();
        let request = tokio::spawn(async move {
            worker
                .request(
                    Method::POST,
                    "timed",
                    "/tickets",
                    Some("member-timeout"),
                    json!({"title":"timeout","assigneeUserId":member.user.to_string()}),
                )
                .await
        });
        gate.observed().await;
        request.await.unwrap().error(503, "PLATFORM_UNAVAILABLE");
        assert!(started.elapsed() < Duration::from_secs(2));
        gate.release.add_permits(1);
        assert_eq!(counts(&f).await, (0, 0));
        assert_eq!(counter(&f, f.identity.tenant).await, None);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn member_wait_holds_no_ticket_lock_or_pool_connection() {
    owned(|f|async move {
        let h=Arc::new(Harness::configured(&f,&[("WORK_ORDERS_DB_MAX_CONNECTIONS","1"),("PLATFORM_AUTH_TIMEOUT_MS","500")]).await);
        let base=h.create("member-wait","before").await;assert_eq!(base.status,201);let id=base.json()["id"].as_str().unwrap().to_owned();
        let member=selected(f.identity,Uuid::new_v4(),f.identity.org);h.add_member(member,"Member");
        let gate=Gate::new();let mut scenario=Scenario::allowed(f.identity);scenario.gate=Some(gate.clone());h.put("waiting",scenario);
        let worker=h.clone();let path=format!("/tickets/{id}");let request=tokio::spawn(async move{
            worker.request(Method::PATCH,"waiting",&path,None,json!({"version":1,"assigneeUserId":member.user.to_string()})).await
        });
        gate.observed().await;assert_eq!(h.pool.size(),1);assert_eq!(h.pool.num_idle(),1,"pre-read lease released before member HTTP");
        let mut pool_probe=tokio::time::timeout(Duration::from_millis(200),h.pool.acquire()).await.expect("pool available during member HTTP").unwrap();pool_probe.execute("SELECT 1").await.unwrap();drop(pool_probe);
        let mut lock_probe=f.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM public.work_orders WHERE id=$1 FOR UPDATE NOWAIT").bind(Uuid::parse_str(&id).unwrap()).fetch_one(&mut *lock_probe).await.expect("ticket has no row lock during member HTTP");lock_probe.rollback().await.unwrap();
        request.await.unwrap().error(503,"PLATFORM_UNAVAILABLE");gate.release.add_permits(1);
        assert_eq!(row(&f,&id).await["version"],1);assert_eq!(row(&f,&id).await["assignee_user_id"],Value::Null);
        let recovered=h.request(Method::PATCH,"writer",&format!("/tickets/{id}"),None,json!({"version":1,"assigneeUserId":member.user.to_string()})).await;assert_eq!(recovered.status,200,"{}",recovered.text());
        println!("WORK_ORDERS_MEMBER_LOCK_PROOF member_http_gate=true application_pool_idle=1 same_ticket_nowait=true timeout_left_row_unchanged=true");
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn pre_read_organization_change_rejected() {
    owned(|f|async move {
        let h=Arc::new(Harness::new(&f).await);let base=h.create("org-race","before").await;assert_eq!(base.status,201);let id=base.json()["id"].as_str().unwrap().to_owned();
        let member=selected(f.identity,Uuid::new_v4(),f.identity.org);h.add_member(member,"Original organization member");
        let gate=Gate::new();let mut scenario=Scenario::allowed(f.identity);scenario.gate=Some(gate.clone());h.put("org-race",scenario);
        let worker=h.clone();let path=format!("/tickets/{id}");let request=tokio::spawn(async move{
            worker.request(Method::PATCH,"org-race",&path,None,json!({"version":1,"title":"must not mutate moved record","assigneeUserId":member.user.to_string()})).await
        });
        gate.observed().await;let new_org=Uuid::new_v4();let mut injection=f.pool.begin().await.unwrap();
        // This fault is confined to the owned test database. Production's
        // immutable-identity trigger normally prevents this organization move.
        injection.execute("ALTER TABLE public.work_orders DISABLE TRIGGER work_order_identity_immutable").await.unwrap();
        sqlx::query("UPDATE public.work_orders SET organization_id=$1 WHERE id=$2").bind(new_org).bind(Uuid::parse_str(&id).unwrap()).execute(&mut *injection).await.unwrap();
        injection.execute("ALTER TABLE public.work_orders ENABLE TRIGGER work_order_identity_immutable").await.unwrap();injection.commit().await.unwrap();
        gate.release.add_permits(1);request.await.unwrap().error(409,"VERSION_CONFLICT");
        let after=row(&f,&id).await;assert_eq!(after["organization_id"],new_org.to_string());assert_eq!(after["version"],1);assert_eq!(after["title"],"before");assert_eq!(after["assignee_user_id"],Value::Null);
        let enabled:String=sqlx::query_scalar("SELECT tgenabled::text FROM pg_trigger WHERE tgrelid='public.work_orders'::regclass AND tgname='work_order_identity_immutable'").fetch_one(&f.pool).await.unwrap();assert_eq!(enabled,"O");
        println!("WORK_ORDERS_MEMBER_ORGANIZATION_PROOF pre_read_org_changed=true version_unchanged=true assignment_rejected=true identity_trigger_reenabled=true");
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn member_prevalidation_version_race_and_expiry_rollback() {
    owned(|f|async move {
        let h=Arc::new(Harness::configured(&f,&[("WORK_ORDERS_DB_LOCK_TIMEOUT_MS","5000")]).await);
        let member=selected(f.identity,Uuid::new_v4(),f.identity.org);h.add_member(member,"Member");
        for old_assignee_race in [false,true] {
            let key=if old_assignee_race{"old-assignee-race"}else{"version-race"};let base=h.create(key,"before").await;assert_eq!(base.status,201);let id=base.json()["id"].as_str().unwrap().to_owned();
            let gate=Gate::new();let mut scenario=Scenario::allowed(f.identity);scenario.gate=Some(gate.clone());h.put("racing",scenario);
            let worker=h.clone();let path=format!("/tickets/{id}");let request=tokio::spawn(async move{
                worker.request(Method::PATCH,"racing",&path,None,json!({"version":1,"assigneeUserId":member.user.to_string()})).await
            });
            gate.observed().await;
            if old_assignee_race {
                sqlx::query("UPDATE public.work_orders SET assignee_user_id=$1,assignee_display_name='Concurrent snapshot' WHERE id=$2")
                    .bind(Uuid::new_v4()).bind(Uuid::parse_str(&id).unwrap()).execute(&f.pool).await.unwrap();
            } else {
                sqlx::query("UPDATE public.work_orders SET title='parallel edit',version=2 WHERE id=$1").bind(Uuid::parse_str(&id).unwrap()).execute(&f.pool).await.unwrap();
            }
            gate.release.add_permits(1);request.await.unwrap().error(409,"VERSION_CONFLICT");
            let after=row(&f,&id).await;assert_ne!(after["assignee_user_id"],member.user.to_string());
            assert_eq!(after["version"],if old_assignee_race{1}else{2});
        }
        let timed=Arc::new(Harness::configured(&f,&[("PLATFORM_AUTH_TIMEOUT_MS","500"),("WORK_ORDERS_DB_LOCK_TIMEOUT_MS","5000")]).await);timed.add_member(member,"Member");
        let base=timed.create("expired-member","before").await;assert_eq!(base.status,201);let id=base.json()["id"].as_str().unwrap().to_owned();
        let mut blocker=f.pool.begin().await.unwrap();sqlx::query("SELECT id FROM public.work_orders WHERE id=$1 FOR UPDATE").bind(Uuid::parse_str(&id).unwrap()).fetch_one(&mut *blocker).await.unwrap();
        let slow_member=Gate::new();let mut scenario=Scenario::allowed(f.identity);scenario.gate=Some(slow_member.clone());timed.put("ttl-member",scenario);
        let worker=timed.clone();let path=format!("/tickets/{id}");let request=tokio::spawn(async move{
            worker.request(Method::PATCH,"ttl-member",&path,None,json!({"version":1,"assigneeUserId":member.user.to_string()})).await
        });
        slow_member.observed().await;let started=timed.member_calls().last().unwrap().started;
        // Consume part of the 500 ms budget in the HTTP exchange itself. A
        // mistaken fresh TTL starting at response completion would outlive the
        // point below, and would wrongly allow the mutation after unlocking.
        tokio::time::sleep_until(tokio::time::Instant::from_std(started+Duration::from_millis(200))).await;
        slow_member.release.add_permits(1);wait_for_lock(&f,"FOR UPDATE").await;
        // The independent 28-second authorization context remains valid.
        tokio::time::sleep_until(tokio::time::Instant::from_std(started+Duration::from_millis(550))).await;
        blocker.rollback().await.unwrap();request.await.unwrap().error(503,"MEMBER_VALIDATION_EXPIRED");
        let after=row(&f,&id).await;assert_eq!(after["version"],1);assert_eq!(after["title"],"before");assert_eq!(after["assignee_user_id"],Value::Null);
        println!("WORK_ORDERS_MEMBER_EXPIRY_PROOF assertion_ms=500 http_budget_consumed_ms=200 context_seconds=28 lock_wait_observed=true rollback=true");
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn assignment_scope_does_not_expand_self_access() {
    owned(|f|async move {
        let h=Harness::new(&f).await;let child=Uuid::new_v4();let child_assignee=Uuid::new_v4();let other_owner=Identity{user:Uuid::new_v4(),..f.identity};
        h.put("other-owner",Scenario::allowed(other_owner));h.add_member(f.identity,"Caller as assignee");
        let assigned=h.request(Method::POST,"other-owner","/tickets",Some("not-owned"),json!({"title":"assigned but not owned","assigneeUserId":f.identity.user.to_string()})).await;assert_eq!(assigned.status,201);
        let assigned_id=assigned.json()["id"].as_str().unwrap().to_owned();
        let mut self_only=Scenario::allowed(f.identity);self_only.scope="SELF";self_only.ceiling=jids(&[f.identity.org,child]);h.put("writer",self_only.clone());
        let list=h.get("writer","/tickets").await;assert_eq!(list.status,200);assert_eq!(list.json()["total"],0);
        let export=h.get("writer","/tickets/export.csv").await;assert_eq!(export.status,200);assert_eq!(export.text().matches("\r\n").count(),1);
        h.get("writer",&format!("/tickets/{assigned_id}")).await.error(404,"RESOURCE_NOT_FOUND");
        h.request(Method::PATCH,"writer",&format!("/tickets/{assigned_id}"),None,json!({"version":1,"title":"assignee bypass"})).await.error(404,"RESOURCE_NOT_FOUND");
        h.request(Method::DELETE,"writer",&format!("/tickets/{assigned_id}"),None,json!({"version":1})).await.error(404,"RESOURCE_NOT_FOUND");
        let owned_child=Uuid::new_v4();
        sqlx::query("INSERT INTO public.work_orders(id,number,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,title,assignee_user_id,assignee_display_name) VALUES($1,'HISTORIC-CHILD',$2,$3,'Child',$4,'Owner','Historical child record',$5,'Historic Child Assignee')")
            .bind(owned_child).bind(f.identity.tenant).bind(child).bind(f.identity.user).bind(child_assignee).execute(&f.pool).await.unwrap();
        let path=format!("/tickets/{owned_child}");let before=h.member_calls().len();
        let same=h.request(Method::PATCH,"writer",&path,None,json!({"version":1,"title":"same historical assignment","assigneeUserId":child_assignee.to_string()})).await;
        assert_eq!(same.status,200,"{}",same.text());assert_eq!(same.json()["assigneeDisplayName"],"Historic Child Assignee");assert_eq!(h.member_calls().len(),before);
        let ordinary=h.request(Method::PATCH,"writer",&path,None,json!({"version":2,"title":"ordinary owner edit"})).await;assert_eq!(ordinary.status,200);
        h.request(Method::PATCH,"writer",&path,None,json!({"version":3,"assigneeUserId":null})).await.error(403,"DATA_DENIED");
        h.request(Method::PATCH,"writer",&path,None,json!({"version":3,"assigneeUserId":Uuid::new_v4().to_string()})).await.error(403,"DATA_DENIED");assert_eq!(h.member_calls().len(),before);
        self_only.ceiling=jids(&[f.identity.org]);h.put("writer",self_only);
        h.get("writer",&path).await.error(404,"RESOURCE_NOT_FOUND");
        h.request(Method::PATCH,"writer",&path,None,json!({"version":3,"assigneeUserId":child_assignee.to_string()})).await.error(404,"RESOURCE_NOT_FOUND");
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn current_grant_or_membership_revocation_next_request() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let member = selected(f.identity, Uuid::new_v4(), f.identity.org);
        h.add_member(member, "Initially effective");
        let base = h.create("revocation-base", "before").await;
        assert_eq!(base.status, 201);
        let id = base.json()["id"].as_str().unwrap().to_owned();
        let first = h.get("writer", "/assignees?action=create").await;
        assert_eq!(first.status, 200);
        assert_eq!(first.json()["total"], 1);
        let before = h.member_calls().len();
        let mut denied = Scenario::allowed(f.identity);
        denied.permissions.retain(|p| p != "ticket:create");
        h.put("writer", denied);
        h.get("writer", "/assignees?action=create")
            .await
            .error(403, "OPERATION_DENIED");
        h.request(
            Method::POST,
            "writer",
            "/tickets",
            Some("revoked-create"),
            json!({"title":"denied","assigneeUserId":member.user.to_string()}),
        )
        .await
        .error(403, "OPERATION_DENIED");
        assert_eq!(h.member_calls().len(), before);
        let mut denied = Scenario::allowed(f.identity);
        denied.grants.retain(|a| a != "update");
        h.put("writer", denied);
        h.request(
            Method::PATCH,
            "writer",
            &format!("/tickets/{id}"),
            None,
            json!({"version":1,"assigneeUserId":member.user.to_string()}),
        )
        .await
        .error(403, "DATA_DENIED");
        assert_eq!(h.member_calls().len(), before);
        h.put("writer", Scenario::allowed(f.identity));
        h.state
            .lock()
            .unwrap()
            .member_records
            .get_mut(&(member.tenant, member.org, member.user))
            .unwrap()
            .enabled = false;
        let refreshed = h.get("writer", "/assignees?action=create").await;
        assert_eq!(refreshed.status, 200);
        assert_eq!(refreshed.json()["total"], 0);
        h.request(
            Method::POST,
            "writer",
            "/tickets",
            Some("removed-member"),
            json!({"title":"removed","assigneeUserId":member.user.to_string()}),
        )
        .await
        .error(404, "MEMBER_NOT_FOUND");
        h.request(
            Method::PATCH,
            "writer",
            &format!("/tickets/{id}"),
            None,
            json!({"version":1,"assigneeUserId":member.user.to_string()}),
        )
        .await
        .error(404, "MEMBER_NOT_FOUND");
        assert_eq!(row(&f, &id).await["version"], 1);
        assert_eq!(row(&f, &id).await["assignee_user_id"], Value::Null);
        assert_eq!(counts(&f).await, (1, 1));
    })
    .await;
}
