//! Explicit owned PostgreSQL + controlled platform HTTP fixture write contracts.
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
    io::AsyncReadExt,
    net::TcpListener,
    sync::{Notify, Semaphore},
    task::JoinHandle,
};
use uuid::Uuid;
use work_orders_api::{application, connect_database, initialize_database, Config};

const PREFIX: &str = "/work-orders-api";
const SECRET: &str = "phase18-fixture-secret";
const BASIC: &str = "Basic aW50ZWdyYXRpb246cGhhc2UxOC1maXh0dXJlLXNlY3JldA==";
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
    let name = format!("work_orders_phase18_{}", Uuid::new_v4().simple());
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
            println!("WORK_ORDERS_WRITE_FIXTURE_SETUP_CLEANUP database_removed={removed} pool_closed=true");
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
        "WORK_ORDERS_WRITE_FIXTURE_CLEANUP assertions_ok={} database_removed={} pool_closed=true",
        result.is_ok(),
        removed
    );
    assert!(removed, "owned database cleanup failed");
    if let Err(error) = result {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("owned write task cancelled");
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
    if let Some(gate) = &scenario.gate {
        gate.entered.notify_one();
        let permit = gate.release.acquire().await.expect("fixture release gate");
        permit.forget();
    }
    Json(scenario.context()).into_response()
}

async fn forbidden_directory(State(shared): State<Shared>) -> Response {
    shared.lock().expect("platform state").members += 1;
    // A permissive member fake must never make an unfinished production path pass.
    StatusCode::SERVICE_UNAVAILABLE.into_response()
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
            .route("/api/apps/work-orders/members", any(forbidden_directory))
            .route(
                "/api/apps/work-orders/members/{id}",
                any(forbidden_directory),
            )
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

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn create_requires_valid_header_and_canonical_body() {
    owned(|f|async move {
        let h=Harness::new(&f).await;
        h.request(Method::POST,"writer","/tickets",None,json!({"title":"valid"})).await.error(400,"VALIDATION_ERROR");
        for key in ["".to_owned(),"space key".into(),"bad/key".into(),"x".repeat(129)] {
            h.create(&key,"valid").await.error(400,"VALIDATION_ERROR");
        }
        let response=h.client.post(format!("{}{PREFIX}/tickets",h.origin)).header("origin","null").bearer_auth("writer")
            .header("Idempotency-Key","first").header("Idempotency-Key","second").json(&json!({"title":"valid"})).send().await.unwrap();
        assert_eq!(response.status(),400);
        assert_eq!(counts(&f).await,(0,0));assert_eq!(counter(&f,f.identity.tenant).await,None);
        let first=h.raw(Method::POST,"writer","/tickets",Some("canonical"),br#"{"title":"  canonical title  "}"#.to_vec(),Some("application/json")).await;
        assert_eq!(first.status,201,"{}",first.text());assert_eq!(first.json()["title"],"canonical title");
        assert_eq!(first.json()["description"],"");assert_eq!(first.json()["priority"],"NORMAL");assert_eq!(first.json()["assigneeUserId"],Value::Null);
        assert_eq!(first.json()["organizationName"],"写入组织");assert_eq!(first.json()["ownerDisplayName"],"写入用户");
        let replay=h.raw(Method::POST,"writer","/tickets",Some("canonical"),br#"{"priority":"NORMAL","assigneeUserId":null,"description":"","title":"canonical title"}"#.to_vec(),Some("application/json; charset=utf-8")).await;
        assert_eq!(replay.status,201);assert_eq!(replay.body,first.body);assert_eq!(counts(&f).await,(1,1));
        let mut boundary=br#"{"title":"canonical title"}"#.to_vec();boundary.resize(65536,b' ');
        let boundary=h.raw(Method::POST,"writer","/tickets",Some("canonical"),boundary,Some("application/json")).await;
        assert_eq!(boundary.status,201,"exactly 64 KiB is permitted");assert_eq!(boundary.body,first.body);
        assert_eq!(counter(&f,f.identity.tenant).await,Some(1));h.no_directory();
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn concurrent_create_replays_once_and_conflicting_body() {
    owned(|f| async move {
        let h = Arc::new(Harness::new(&f).await);
        let barrier = Arc::new(tokio::sync::Barrier::new(9));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let h = h.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                h.create("concurrent", "one ticket").await
            }));
        }
        barrier.wait().await;
        let mut original = None;
        for task in tasks {
            let reply = task.await.unwrap();
            assert_eq!(reply.status, 201, "{}", reply.text());
            if let Some(body) = &original {
                assert_eq!(&reply.body, body);
            } else {
                original = Some(reply.body);
            }
        }
        assert_eq!(counts(&f).await, (1, 1));
        assert_eq!(counter(&f, f.identity.tenant).await, Some(1));
        h.create("concurrent", "different ticket")
            .await
            .error(409, "IDEMPOTENCY_CONFLICT");
        assert_eq!(counts(&f).await, (1, 1));
        assert_eq!(counter(&f, f.identity.tenant).await, Some(1));
        h.no_directory();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn committed_response_loss_replays_original_result() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_origin = format!("http://{}", listener.local_addr().unwrap());
        let client = h.client.clone();
        let upstream = format!("{}{PREFIX}/tickets", h.origin);
        let proxy = tokio::spawn(async move {
            let (mut downstream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let n = downstream.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buffer[..n]);
                assert!(request.len() < 16384);
            }
            let body_start = request
                .windows(4)
                .position(|part| part == b"\r\n\r\n")
                .unwrap()
                + 4;
            let head = std::str::from_utf8(&request[..body_start]).unwrap();
            assert!(head.starts_with("POST /work-orders-api/tickets HTTP/1.1\r\n"));
            let headers: HashMap<_, _> = head
                .lines()
                .skip(1)
                .filter_map(|line| line.split_once(':'))
                .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
                .collect();
            let length: usize = headers.get("content-length").unwrap().parse().unwrap();
            assert!(length <= 65536);
            while request.len() - body_start < length {
                let n = downstream.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buffer[..n]);
            }
            let body = request[body_start..body_start + length].to_vec();
            let response = client
                .post(upstream)
                .header("origin", headers.get("origin").unwrap())
                .header("authorization", headers.get("authorization").unwrap())
                .header("Idempotency-Key", headers.get("idempotency-key").unwrap())
                .header("content-type", headers.get("content-type").unwrap())
                .body(body)
                .send()
                .await
                .unwrap();
            let reply = Reply {
                status: response.status().as_u16(),
                headers: response.headers().clone(),
                body: response.bytes().await.unwrap().to_vec(),
            };
            assert_eq!(
                reply.status, 201,
                "upstream must finish committing before response loss"
            );
            drop(downstream);
            reply
        });
        let lost = h
            .client
            .post(format!("{proxy_origin}{PREFIX}/tickets"))
            .header("origin", "null")
            .bearer_auth("writer")
            .header("Idempotency-Key", "lost-response")
            .json(&json!({"title":"committed ticket"}))
            .send()
            .await;
        assert!(lost.is_err(), "downstream receives no successful response");
        let original = proxy.await.unwrap();
        assert_eq!(counts(&f).await, (1, 1));
        let replay = h.create("lost-response", "committed ticket").await;
        assert_eq!(replay.status, 201);
        assert_eq!(replay.body, original.body);
        assert_eq!(counter(&f, f.identity.tenant).await, Some(1));
        let id = replay.json()["id"].as_str().unwrap().to_owned();
        let patched = h
            .request(
                Method::PATCH,
                "writer",
                &format!("/tickets/{id}"),
                None,
                json!({"version":1,"title":"later title"}),
            )
            .await;
        assert_eq!(patched.status, 200);
        let replay = h.create("lost-response", "committed ticket").await;
        assert_eq!(
            replay.body, original.body,
            "receipt preserves the original result snapshot"
        );
        h.no_directory();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn receipt_identity_domains_and_revocation() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let body = json!({"title":"same domain body"});
        let mut ids = std::collections::BTreeSet::new();
        let other_tenant = Uuid::new_v4();
        for (token, identity) in [
            ("writer", f.identity),
            (
                "new-org",
                Identity {
                    org: Uuid::new_v4(),
                    ..f.identity
                },
            ),
            (
                "new-user",
                Identity {
                    user: Uuid::new_v4(),
                    ..f.identity
                },
            ),
            (
                "new-tenant",
                Identity {
                    tenant: other_tenant,
                    ..f.identity
                },
            ),
        ] {
            h.put(token, Scenario::allowed(identity));
            let reply = h
                .request(
                    Method::POST,
                    token,
                    "/tickets",
                    Some("domain-key"),
                    body.clone(),
                )
                .await;
            assert_eq!(reply.status, 201, "{}", reply.text());
            assert!(ids.insert(reply.json()["id"].as_str().unwrap().to_owned()));
            assert_eq!(reply.json()["tenantId"], identity.tenant.to_string());
            assert_eq!(reply.json()["organizationId"], identity.org.to_string());
            assert_eq!(reply.json()["ownerUserId"], identity.user.to_string());
        }
        assert_eq!(counts(&f).await, (4, 4));
        assert_eq!(counter(&f, f.identity.tenant).await, Some(3));
        assert_eq!(counter(&f, other_tenant).await, Some(1));
        let mut create_only = Scenario::allowed(f.identity);
        create_only.permissions = vec!["ticket:create".into()];
        create_only.grants = vec!["create".into()];
        h.put("writer", create_only);
        let own_replay = h
            .request(
                Method::POST,
                "writer",
                "/tickets",
                Some("domain-key"),
                body.clone(),
            )
            .await;
        assert_eq!(
            own_replay.status, 201,
            "receipt recovery requires current create authorization"
        );
        assert_eq!(
            own_replay.json()["ownerUserId"],
            f.identity.user.to_string()
        );
        let mut denied = Scenario::allowed(f.identity);
        denied.permissions.retain(|p| p != "ticket:create");
        h.put("writer", denied);
        h.request(
            Method::POST,
            "writer",
            "/tickets",
            Some("domain-key"),
            body.clone(),
        )
        .await
        .error(403, "OPERATION_DENIED");
        let mut denied = Scenario::allowed(f.identity);
        denied.grants.retain(|a| a != "create");
        h.put("writer", denied);
        h.request(Method::POST, "writer", "/tickets", Some("domain-key"), body)
            .await
            .error(403, "DATA_DENIED");
        assert_eq!(counts(&f).await, (4, 4));
        h.no_directory();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn patch_version_race_and_status_transitions() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let created = h.create("state", "initial").await;
        assert_eq!(created.status, 201);
        let id = created.json()["id"].as_str().unwrap().to_owned();
        let path = format!("/tickets/{id}");
        let (a, b) = tokio::join!(
            h.request(
                Method::PATCH,
                "writer",
                &path,
                None,
                json!({"version":1,"title":"A"})
            ),
            h.request(
                Method::PATCH,
                "writer",
                &path,
                None,
                json!({"version":1,"title":"B"})
            )
        );
        assert_eq!(
            [a.status, b.status].iter().filter(|s| **s == 200).count(),
            1
        );
        let loser = if a.status == 409 { a } else { b };
        loser.error(409, "VERSION_CONFLICT");
        assert_eq!(row(&f, &id).await["version"], 2);
        h.request(
            Method::PATCH,
            "writer",
            &path,
            None,
            json!({"version":2,"status":"RESOLVED"}),
        )
        .await
        .error(400, "VALIDATION_ERROR");
        assert_eq!(row(&f, &id).await["version"], 2);
        for (version, status) in [(2, "IN_PROGRESS"), (3, "RESOLVED"), (4, "IN_PROGRESS")] {
            let reply = h
                .request(
                    Method::PATCH,
                    "writer",
                    &path,
                    None,
                    json!({"version":version,"status":status}),
                )
                .await;
            assert_eq!(reply.status, 200, "{}", reply.text());
            assert_eq!(reply.json()["version"], version + 1);
            assert_eq!(reply.json()["status"], status);
        }
        h.no_directory();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn patch_delete_version_race_and_deleted_visibility() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let created = h.create("deleted", "initial").await;
        assert_eq!(created.status, 201);
        let id = created.json()["id"].as_str().unwrap().to_owned();
        let path = format!("/tickets/{id}");
        let (patch, delete) = tokio::join!(
            h.request(
                Method::PATCH,
                "writer",
                &path,
                None,
                json!({"version":1,"title":"changed"})
            ),
            h.request(Method::DELETE, "writer", &path, None, json!({"version":1}))
        );
        assert_eq!(
            [patch.status, delete.status]
                .iter()
                .filter(|s| **s == 200)
                .count(),
            1
        );
        let (loser, deleted) = if patch.status == 200 {
            (delete, false)
        } else {
            (patch, true)
        };
        if loser.status == 409 {
            loser.error(409, "VERSION_CONFLICT");
        } else {
            loser.error(404, "RESOURCE_NOT_FOUND");
        }
        if !deleted {
            let reply = h
                .request(Method::DELETE, "writer", &path, None, json!({"version":2}))
                .await;
            assert_eq!(reply.status, 200);
            assert_eq!(reply.json(), json!({"ok":true}));
        }
        h.request(Method::GET, "writer", &path, None, Value::Null)
            .await
            .error(404, "RESOURCE_NOT_FOUND");
        let list = h
            .request(Method::GET, "writer", "/tickets", None, Value::Null)
            .await;
        assert_eq!(list.status, 200);
        assert_eq!(list.json()["total"], 0);
        let export = h
            .request(
                Method::GET,
                "writer",
                "/tickets/export.csv",
                None,
                Value::Null,
            )
            .await;
        assert_eq!(export.status, 200);
        assert_eq!(export.text().matches("\r\n").count(), 1);
        let replay = h.create("deleted", "initial").await;
        assert_eq!(replay.status, 201);
        assert_eq!(replay.body, created.body);
        assert_eq!(counts(&f).await, (1, 1));
        h.no_directory();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn write_validation_preserves_identity_and_rejects_duplicate_json() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let created = h.create("validation", "immutable").await;
        assert_eq!(created.status, 201);
        let id = created.json()["id"].as_str().unwrap().to_owned();
        let path = format!("/tickets/{id}");
        let original = row(&f, &id).await;
        for (method, target, body) in [
            (Method::POST, "/tickets".to_owned(), json!(["x"])),
            (Method::PATCH, path.clone(), json!([1, "x"])),
            (Method::DELETE, path.clone(), json!([1])),
        ] {
            h.request(method, "writer", &target, Some("array-body"), body)
                .await
                .error(400, "VALIDATION_ERROR");
        }
        for body in [
            json!({"version":1,"tenantId":Uuid::new_v4().to_string()}),
            json!({"version":1,"organizationId":Uuid::new_v4().to_string()}),
            json!({"version":1,"ownerUserId":Uuid::new_v4().to_string()}),
            json!({"version":1,"number":"SPOOF"}),
            json!({"version":1,"organizationName":"SPOOF"}),
            json!({"version":1,"unknown":1}),
            json!({"version":1}),
            json!({"version":0,"title":"bad"}),
            json!({"version":1.5,"title":"bad"}),
            json!({"version":"1","title":"bad"}),
            json!({"version":2147483647_i64,"title":"bad"}),
        ] {
            h.request(Method::PATCH, "writer", &path, None, body)
                .await
                .error(400, "VALIDATION_ERROR");
        }
        for bytes in [
            br#"{"version":1,"title":"a","title":"b"}"#.to_vec(),
            br#"{"version":1,"version":1,"title":"a"}"#.to_vec(),
            b"[]".to_vec(),
            b"null".to_vec(),
            b"{not-json".to_vec(),
            vec![b'{', b'"', b'x', b'"', b':', b'"', 0xff, b'"', b'}'],
        ] {
            h.raw(
                Method::PATCH,
                "writer",
                &path,
                None,
                bytes,
                Some("application/json"),
            )
            .await
            .error(400, "VALIDATION_ERROR");
        }
        h.raw(
            Method::POST,
            "writer",
            "/tickets",
            Some("duplicate"),
            br#"{"title":"a","title":"b"}"#.to_vec(),
            Some("application/json"),
        )
        .await
        .error(400, "VALIDATION_ERROR");
        h.raw(
            Method::POST,
            "writer",
            "/tickets",
            Some("wrong-type"),
            br#"{"title":"valid"}"#.to_vec(),
            Some("text/plain"),
        )
        .await
        .error(400, "VALIDATION_ERROR");
        h.raw(
            Method::POST,
            "writer",
            "/tickets",
            Some("too-large"),
            vec![b' '; 65537],
            Some("application/json"),
        )
        .await
        .error(413, "REQUEST_LIMIT");
        for (method, target, body) in [
            (
                Method::POST,
                "/tickets".to_owned(),
                json!({"title":"query"}),
            ),
            (
                Method::PATCH,
                path.clone(),
                json!({"version":1,"title":"query"}),
            ),
            (Method::DELETE, path.clone(), json!({"version":1})),
        ] {
            h.request(
                method,
                "writer",
                &format!("{target}?tenantId={}", f.identity.tenant),
                Some("query"),
                body,
            )
            .await
            .error(400, "VALIDATION_ERROR");
        }
        h.request(
            Method::DELETE,
            "writer",
            &path,
            None,
            json!({"version":1,"title":"unknown"}),
        )
        .await
        .error(400, "VALIDATION_ERROR");
        assert_eq!(row(&f, &id).await, original);
        assert_eq!(counts(&f).await, (1, 1));
        h.no_directory();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn nonempty_assignee_requires_available_validated_directory() {
    owned(|f|async move {
        let h=Harness::new(&f).await;let assignee=Uuid::new_v4().to_string();
        h.request(Method::POST,"writer","/tickets",Some("assigned-create"),json!({"title":"assigned","assigneeUserId":assignee})).await.error(503,"PLATFORM_UNAVAILABLE");
        assert_eq!(h.state.lock().unwrap().members,1);
        assert_eq!(counts(&f).await,(0,0));assert_eq!(counter(&f,f.identity.tenant).await,None);
        let created=h.request(Method::POST,"writer","/tickets",Some("null-create"),json!({"title":"unassigned","assigneeUserId":null})).await;assert_eq!(created.status,201);
        let id=created.json()["id"].as_str().unwrap().to_owned();let path=format!("/tickets/{id}");
        h.request(Method::PATCH,"writer",&path,None,json!({"version":1,"assigneeUserId":assignee})).await.error(503,"PLATFORM_UNAVAILABLE");
        assert_eq!(h.state.lock().unwrap().members,2);
        sqlx::query("UPDATE public.work_orders SET assignee_user_id=$1,assignee_display_name='历史处理人' WHERE id=$2")
            .bind(Uuid::parse_str(&assignee).unwrap()).bind(Uuid::parse_str(&id).unwrap()).execute(&f.pool).await.unwrap();
        let retained=h.request(Method::PATCH,"writer",&path,None,json!({"version":1,"assigneeUserId":assignee})).await;
        assert_eq!(retained.status,200);assert_eq!(retained.json()["assigneeDisplayName"],"历史处理人");assert_eq!(retained.json()["version"],2);
        h.request(Method::POST,"writer","/tickets",Some("invalid-member"),json!({"title":"invalid","assigneeUserId":"invalid"})).await.error(400,"VALIDATION_ERROR");
        h.request(Method::PATCH,"writer",&path,None,json!({"version":2,"assigneeUserId":"invalid"})).await.error(400,"VALIDATION_ERROR");
        assert_eq!(h.state.lock().unwrap().members,2,"unchanged and malformed assignments do not call the directory");
        let cleared=h.request(Method::PATCH,"writer",&path,None,json!({"version":2,"assigneeUserId":null})).await;assert_eq!(cleared.status,200);assert_eq!(cleared.json()["assigneeUserId"],Value::Null);
        let preflight=h.client.request(Method::OPTIONS,format!("{}{PREFIX}/tickets",h.origin)).header("origin","null")
            .header("access-control-request-method","POST").header("access-control-request-headers","Authorization, Content-Type, Idempotency-Key").send().await.unwrap();
        assert_eq!(preflight.status(),204);assert_eq!(preflight.headers().get("access-control-allow-origin").unwrap(),"null");
        assert!(preflight.headers().get("access-control-allow-headers").unwrap().to_str().unwrap().to_ascii_lowercase().contains("idempotency-key"));
        assert_eq!(h.state.lock().unwrap().members,2);
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn authorization_wait_holds_no_pool_connection_or_row_lock() {
    owned(|f|async move {
        let h=Arc::new(Harness::configured(&f,&[("WORK_ORDERS_DB_MAX_CONNECTIONS","1")]).await);
        let created=h.create("http-wait","before").await;assert_eq!(created.status,201);let id=created.json()["id"].as_str().unwrap().to_owned();
        let gate=Gate::new();let mut scenario=Scenario::allowed(f.identity);scenario.gate=Some(gate.clone());h.put("delayed",scenario);
        let worker=h.clone();let path=format!("/tickets/{id}");
        let request=tokio::spawn(async move {worker.request(Method::PATCH,"delayed",&path,None,json!({"version":1,"title":"after"})).await});
        gate.observed().await;
        assert_eq!(h.pool.size(),1);assert_eq!(h.pool.num_idle(),1,"no application connection held while context HTTP is pending");
        let mut probe=tokio::time::timeout(Duration::from_millis(500),h.pool.acquire()).await.expect("application pool remains available").unwrap();
        probe.execute("SELECT 1").await.unwrap();drop(probe);
        let mut probe=f.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM public.work_orders WHERE id=$1 FOR UPDATE NOWAIT").bind(Uuid::parse_str(&id).unwrap()).fetch_one(&mut *probe).await.expect("no row lock while external HTTP pending");
        probe.rollback().await.unwrap();gate.release.add_permits(1);
        let reply=request.await.unwrap();assert_eq!(reply.status,200,"{}",reply.text());assert_eq!(reply.json()["version"],2);
        println!("WORK_ORDERS_WRITE_BUDGET external_http_gate=true application_pool_idle=1 row_lock_nowait=true");h.no_directory();
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn expired_authorization_during_row_wait_rolls_back() {
    owned(|f| async move {
        let h =
            Arc::new(Harness::configured(&f, &[("WORK_ORDERS_DB_LOCK_TIMEOUT_MS", "5000")]).await);
        let created = h.create("expired", "before").await;
        assert_eq!(created.status, 201);
        let id = created.json()["id"].as_str().unwrap().to_owned();
        let mut blocker = f.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM public.work_orders WHERE id=$1 FOR UPDATE")
            .bind(Uuid::parse_str(&id).unwrap())
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let expires = Utc::now() + chrono::Duration::milliseconds(800);
        let mut scenario = Scenario::allowed(f.identity);
        scenario.expires_at = Some(expires);
        h.put("expiring", scenario);
        let worker = h.clone();
        let path = format!("/tickets/{id}");
        let request = tokio::spawn(async move {
            worker
                .request(
                    Method::PATCH,
                    "expiring",
                    &path,
                    None,
                    json!({"version":1,"title":"must roll back"}),
                )
                .await
        });
        wait_for_lock(&f, "FOR UPDATE").await;
        let remaining = (expires.timestamp_millis() - Utc::now().timestamp_millis()).max(0) as u64;
        // Wait for the known authorization expiry only after observing the actual lock wait.
        tokio::time::sleep(Duration::from_millis(remaining + 30)).await;
        blocker.rollback().await.unwrap();
        request.await.unwrap().error(401, "INVALID_CONTEXT");
        let unchanged = row(&f, &id).await;
        assert_eq!(unchanged["title"], "before");
        assert_eq!(unchanged["version"], 1);
        assert_eq!(counts(&f).await, (1, 1));
        h.no_directory();
    })
    .await;
}

async fn seed_counter(f: &Fixture, value: i64) {
    sqlx::query("INSERT INTO public.work_order_counters(tenant_id,next_number) VALUES($1,$2)")
        .bind(f.identity.tenant)
        .bind(value)
        .execute(&f.pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn database_lock_timeout_rolls_back() {
    owned(|f| async move {
        seed_counter(&f, 41).await;
        let h =
            Arc::new(Harness::configured(&f, &[("WORK_ORDERS_DB_LOCK_TIMEOUT_MS", "250")]).await);
        let mut blocker = f.pool.begin().await.unwrap();
        sqlx::query(
            "SELECT tenant_id FROM public.work_order_counters WHERE tenant_id=$1 FOR UPDATE",
        )
        .bind(f.identity.tenant)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
        let worker = h.clone();
        let started = Instant::now();
        let request = tokio::spawn(async move { worker.create("lock-timeout", "bounded").await });
        wait_for_lock(&f, "work_order_counters").await;
        request.await.unwrap().error(503, "DATABASE_UNAVAILABLE");
        assert!(started.elapsed() < Duration::from_secs(3));
        blocker.rollback().await.unwrap();
        assert_eq!(counts(&f).await, (0, 0));
        assert_eq!(counter(&f, f.identity.tenant).await, Some(41));
        assert_eq!(h.create("lock-timeout", "bounded").await.status, 201);
        assert_eq!(counter(&f, f.identity.tenant).await, Some(42));
        h.no_directory();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn database_acquire_timeout_is_bounded() {
    owned(|f| async move {
        let h = Harness::configured(
            &f,
            &[
                ("WORK_ORDERS_DB_MAX_CONNECTIONS", "1"),
                ("WORK_ORDERS_DB_ACQUIRE_TIMEOUT_MS", "100"),
            ],
        )
        .await;
        let held = h.pool.acquire().await.unwrap();
        let started = Instant::now();
        h.create("acquire-timeout", "bounded")
            .await
            .error(503, "DATABASE_UNAVAILABLE");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            h.state.lock().unwrap().contexts > 0,
            "authorization finished before waiting for the pool"
        );
        drop(held);
        assert_eq!(counts(&f).await, (0, 0));
        assert_eq!(counter(&f, f.identity.tenant).await, None);
        assert_eq!(h.create("acquire-timeout", "bounded").await.status, 201);
        h.no_directory();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn database_statement_timeout_rolls_back() {
    owned(|f|async move {
        f.pool.execute("CREATE FUNCTION phase18_slow_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(2); RETURN NEW; END $$; CREATE TRIGGER phase18_slow_insert BEFORE INSERT ON public.work_orders FOR EACH ROW EXECUTE FUNCTION phase18_slow_insert()")
            .await.expect("owned statement timeout injection");
        let h=Harness::configured(&f,&[("WORK_ORDERS_DB_STATEMENT_TIMEOUT_MS","100")]).await;let started=Instant::now();
        h.create("statement-timeout","bounded").await.error(503,"DATABASE_UNAVAILABLE");assert!(started.elapsed()<Duration::from_secs(2));
        assert_eq!(counts(&f).await,(0,0));assert_eq!(counter(&f,f.identity.tenant).await,None);
        f.pool.execute("DROP TRIGGER phase18_slow_insert ON public.work_orders; DROP FUNCTION phase18_slow_insert()").await.unwrap();
        assert_eq!(h.create("statement-timeout","bounded").await.status,201);assert_eq!(counter(&f,f.identity.tenant).await,Some(1));h.no_directory();
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn request_deadline_cancels_wait_without_late_commit() {
    owned(|f|async move {
        seed_counter(&f,41).await;
        let h=Arc::new(Harness::configured(&f,&[("WORK_ORDERS_DB_MAX_CONNECTIONS","1"),("WORK_ORDERS_DB_LOCK_TIMEOUT_MS","30000"),("WORK_ORDERS_DB_STATEMENT_TIMEOUT_MS","30000")]).await);
        let mut blocker=f.pool.begin().await.unwrap();sqlx::query("SELECT tenant_id FROM public.work_order_counters WHERE tenant_id=$1 FOR UPDATE").bind(f.identity.tenant).fetch_one(&mut *blocker).await.unwrap();
        let worker=h.clone();let started=Instant::now();let request=tokio::spawn(async move {worker.create("request-deadline","must not commit late").await});
        wait_for_lock(&f,"work_order_counters").await;
        let reply=request.await.unwrap();reply.error(503,"CREATE_RESULT_UNKNOWN");let elapsed=started.elapsed();
        assert!(elapsed>=Duration::from_secs(14));assert!(elapsed<Duration::from_secs(19));
        blocker.rollback().await.unwrap();
        let mut drained=tokio::time::timeout(Duration::from_secs(5),h.pool.acquire()).await.expect("cancelled transaction releases the pool").unwrap();drained.execute("SELECT 1").await.unwrap();drop(drained);
        assert_eq!(counts(&f).await,(0,0));assert_eq!(counter(&f,f.identity.tenant).await,Some(41));
        let retry=h.create("request-deadline","must not commit late").await;assert_eq!(retry.status,201,"{}",retry.text());
        assert_eq!(counts(&f).await,(1,1));assert_eq!(counter(&f,f.identity.tenant).await,Some(42));
        println!("WORK_ORDERS_WRITE_BUDGET request_deadline_ms={} late_commit=false retry_created_once=true",elapsed.as_millis());h.no_directory();
    }).await;
}
