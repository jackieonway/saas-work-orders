//! Owned business PostgreSQL and controlled platform HTTP reminder contracts.
//! The controlled publication ledger is not a real platform database or inbox.
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
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, Semaphore},
    task::{JoinHandle, JoinSet},
};
use uuid::Uuid;
use work_orders_api::{application, connect_database, initialize_database, Config};

const PREFIX: &str = "/work-orders-api";
const SECRET: &str = "phase20-fixture-secret";
const BASIC: &str = "Basic aW50ZWdyYXRpb246cGhhc2UyMC1maXh0dXJlLXNlY3JldA==";

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
    let name = format!("work_orders_phase20_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&admin)
        .await
        .expect("create owned database");
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
            println!("WORK_ORDERS_REMINDER_FIXTURE_SETUP_CLEANUP database_removed={removed} pool_closed=true");
            assert!(removed, "owned setup cleanup failed");
            panic!("owned observer pool setup failed");
        }
    };
    let pools = Arc::new(Mutex::new(vec![pool.clone()]));
    let f = Fixture {
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
        initialize_database(&f.pool)
            .await
            .expect("current business migrations");
        case(f).await;
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
    println!("WORK_ORDERS_REMINDER_FIXTURE_CLEANUP assertions_ok={} database_removed={} pool_closed=true", result.is_ok(), removed);
    assert!(removed, "owned database cleanup failed");
    if let Err(error) = result {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("owned reminder task cancelled");
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
            .expect("controlled publication gate reached");
    }
    async fn wait(&self) {
        self.entered.notify_one();
        self.release
            .acquire()
            .await
            .expect("controlled publication release")
            .forget();
    }
}

#[derive(Clone, Default)]
enum PublishMode {
    #[default]
    Normal,
    Status(u16),
    Patch(Value),
    Raw(String),
    Html,
    Oversize,
    Redirect,
    DropAfterPersist,
    GateAfterPersist(Gate),
}

#[derive(Clone)]
struct Scenario {
    identity: Identity,
    permissions: Vec<String>,
    ticket_scope: &'static str,
    ticket_orgs: Vec<Uuid>,
    ticket_grant: bool,
    notification_scope: &'static str,
    notification_orgs: Vec<Uuid>,
    notification_grant: bool,
    ceiling: Value,
    expires_at: Option<chrono::DateTime<Utc>>,
    context_patch: Value,
    active: bool,
    mode: PublishMode,
}

impl Scenario {
    fn allowed(identity: Identity) -> Self {
        Self {
            identity,
            permissions: vec!["ticket:read".into(), "notification:publish".into()],
            ticket_scope: "TENANT",
            ticket_orgs: vec![],
            ticket_grant: true,
            notification_scope: "TENANT",
            notification_orgs: vec![],
            notification_grant: true,
            ceiling: Value::Null,
            expires_at: None,
            context_patch: json!({}),
            active: true,
            mode: PublishMode::Normal,
        }
    }
    fn introspection(&self) -> Value {
        json!({"active":self.active,"aud":"work-orders","sub":self.identity.user.to_string(),
            "tenant_id":self.identity.tenant.to_string(),"organization_id":self.identity.org.to_string(),
            "exp":Utc::now().timestamp()+600,"token_type":"Bearer"})
    }
    fn context(&self) -> Value {
        let mut grants = vec![];
        for (include, resource, action, scope, organizations) in [
            (
                self.ticket_grant,
                "ticket",
                "read",
                self.ticket_scope,
                &self.ticket_orgs,
            ),
            (
                self.notification_grant,
                "notification",
                "publish",
                self.notification_scope,
                &self.notification_orgs,
            ),
        ] {
            if include {
                grants.push(json!({"resource":resource,"action":action,"scope":scope,
                "organizationIds":organizations.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                "tenantId":self.identity.tenant.to_string(),"subjectUserId":self.identity.user.to_string()}));
            }
        }
        let expiry = self
            .expires_at
            .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(28));
        let mut value = json!({"contractVersion":"1.0","appCode":"work-orders","contextVersion":"b".repeat(64),
            "expiresAt":expiry.to_rfc3339_opts(SecondsFormat::Millis,true),
            "user":{"id":self.identity.user.to_string(),"username":"reminder-user","displayName":"提醒用户"},
            "tenant":{"id":self.identity.tenant.to_string()},"organization":{"id":self.identity.org.to_string(),"name":"提醒组织"},
            "application":{"id":"33333333-3333-4333-8333-333333333333"},"organizationScopeIds":self.ceiling,
            "permissions":self.permissions,"dataGrants":grants});
        for (key, patch) in self.context_patch.as_object().unwrap() {
            value[key] = patch.clone();
        }
        value
    }
}

#[derive(Clone)]
struct PublicationCall {
    token: String,
    request_id: String,
    payload: Value,
}

#[derive(Default)]
struct PlatformState {
    scenarios: HashMap<String, Scenario>,
    introspections: usize,
    contexts: usize,
    calls: Vec<PublicationCall>,
    publications: HashMap<(Uuid, Uuid, Uuid, String), (Value, Value)>,
    redirect_hits: usize,
}

type Shared = Arc<Mutex<PlatformState>>;

async fn introspect(State(shared): State<Shared>, request: Request) -> Response {
    assert_eq!(request.headers().get("authorization").unwrap(), BASIC);
    assert_eq!(
        request.headers().get("content-type").unwrap(),
        "application/x-www-form-urlencoded"
    );
    let bytes = to_bytes(request.into_body(), 16384).await.unwrap();
    let fields: Vec<_> = url::form_urlencoded::parse(&bytes).into_owned().collect();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].0, "token");
    let scenario = {
        let mut state = shared.lock().unwrap();
        state.introspections += 1;
        state.scenarios.get(&fields[0].1).cloned()
    };
    match scenario {
        Some(scenario) => Json(scenario.introspection()).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

fn token(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned()
}

async fn context(State(shared): State<Shared>, request: Request) -> Response {
    let token = token(request.headers());
    let scenario = {
        let mut state = shared.lock().unwrap();
        state.contexts += 1;
        state.scenarios.get(&token).cloned()
    };
    match scenario {
        Some(scenario) => Json(scenario.context()).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn publish(State(shared): State<Shared>, request: Request) -> Response {
    assert_eq!(request.method(), Method::POST);
    assert_eq!(
        request.headers().get("content-type").unwrap(),
        "application/json"
    );
    let token = token(request.headers());
    let request_id = request
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(Uuid::parse_str(&request_id).is_ok());
    let body = to_bytes(request.into_body(), 65536).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).expect("outgoing publication JSON");
    let scenario = {
        let mut state = shared.lock().unwrap();
        state.calls.push(PublicationCall {
            token: token.clone(),
            request_id,
            payload: payload.clone(),
        });
        state.scenarios.get(&token).cloned()
    };
    let Some(scenario) = scenario else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if let PublishMode::Status(status) = scenario.mode {
        if status == 204 {
            return StatusCode::NO_CONTENT.into_response();
        }
        return (
            StatusCode::from_u16(status).unwrap(),
            Json(json!({"message":SECRET,"code":"UNTRUSTED_UPSTREAM_CODE"})),
        )
            .into_response();
    }
    let key = payload["idempotencyKey"]
        .as_str()
        .expect("publication event key")
        .to_owned();
    let namespace = (
        scenario.identity.tenant,
        scenario.identity.org,
        scenario.identity.user,
        key,
    );
    let mut result = {
        let mut state = shared.lock().unwrap();
        if let Some((prior, result)) = state.publications.get(&namespace) {
            if prior != &payload {
                return (StatusCode::CONFLICT, Json(json!({"message":SECRET}))).into_response();
            }
            let mut result = result.clone();
            result["duplicate"] = json!(true);
            result
        } else {
            let result = json!({"publicationId":Uuid::new_v4().to_string(),"recipientCount":1,"deliveryCount":1,
                "duplicate":false,"createdAt":Utc::now().to_rfc3339_opts(SecondsFormat::Millis,true)});
            state
                .publications
                .insert(namespace, (payload, result.clone()));
            result
        }
    };
    match scenario.mode {
        PublishMode::Normal | PublishMode::DropAfterPersist => Json(result).into_response(),
        PublishMode::Status(_) => unreachable!("status returned before fixture persistence"),
        PublishMode::Patch(patch) => {
            for (key, value) in patch.as_object().unwrap() {
                result[key] = value.clone();
            }
            Json(result).into_response()
        }
        PublishMode::Raw(body) => ([("content-type", "application/json")], body).into_response(),
        PublishMode::Html => (
            [("content-type", "text/html")],
            format!("<html>{SECRET}</html>"),
        )
            .into_response(),
        PublishMode::Oversize => {
            result["padding"] = json!("x".repeat(300_000));
            Json(result).into_response()
        }
        PublishMode::Redirect => (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "/redirect-trap")],
            "",
        )
            .into_response(),
        PublishMode::GateAfterPersist(gate) => {
            gate.wait().await;
            Json(result).into_response()
        }
    }
}

async fn trap(State(shared): State<Shared>) -> Response {
    shared.lock().unwrap().redirect_hits += 1;
    Json(json!({"unexpected":true})).into_response()
}

// A local HTTP proxy makes post-persistence transport loss observable without
// adding a stream/dependency or treating a malformed successful body as a drop.
async fn proxy_connection(mut socket: TcpStream, upstream: String, state: Shared) {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !bytes.windows(4).any(|chunk| chunk == b"\r\n\r\n") {
        let n = socket.read(&mut buffer).await.unwrap();
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&buffer[..n]);
        assert!(bytes.len() < 32768);
    }
    let body_start = bytes
        .windows(4)
        .position(|chunk| chunk == b"\r\n\r\n")
        .unwrap()
        + 4;
    let head = std::str::from_utf8(&bytes[..body_start]).unwrap();
    let first: Vec<_> = head.lines().next().unwrap().split_whitespace().collect();
    let method = Method::from_bytes(first[0].as_bytes()).unwrap();
    let path = first[1].to_owned();
    let headers: HashMap<String, String> = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .map(|raw| raw.parse().unwrap())
        .unwrap_or(0);
    assert!(length <= 65536);
    while bytes.len() - body_start < length {
        let n = socket.read(&mut buffer).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut outgoing = client
        .request(method, format!("{upstream}{path}"))
        .body(bytes[body_start..body_start + length].to_vec());
    for header in ["authorization", "content-type", "x-request-id", "accept"] {
        if let Some(value) = headers.get(header) {
            outgoing = outgoing.header(header, value);
        }
    }
    let response = outgoing.send().await.unwrap();
    let status = response.status().as_u16();
    let response_headers = response.headers().clone();
    let body = response.bytes().await.unwrap();
    let dropped = path == "/api/apps/work-orders/notifications" && status == 200 && {
        let token = headers
            .get("authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default();
        state
            .lock()
            .unwrap()
            .scenarios
            .get(token)
            .is_some_and(|s| matches!(s.mode, PublishMode::DropAfterPersist))
    };
    if dropped {
        return;
    }
    let mut output = format!(
        "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for header in ["content-type", "location"] {
        if let Some(value) = response_headers.get(header) {
            output.push_str(&format!("{header}: {}\r\n", value.to_str().unwrap()));
        }
    }
    output.push_str("\r\n");
    // After an intentional client timeout, a closed socket is expected.
    if socket.write_all(output.as_bytes()).await.is_ok() {
        let _ = socket.write_all(&body).await;
    }
}

struct Harness {
    client: reqwest::Client,
    origin: String,
    pool: PgPool,
    config: Config,
    state: Shared,
    tasks: Vec<JoinHandle<std::io::Result<()>>>,
    proxy: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        self.proxy.abort();
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
        std::str::from_utf8(&self.body).unwrap()
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
        assert!(!self.text().contains("UNTRUSTED_UPSTREAM_CODE"));
    }
}

async fn serve_business(
    config: &Config,
    pool: PgPool,
) -> (String, JoinHandle<std::io::Result<()>>) {
    let app = application(config, pool).expect("reminder application");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    (
        origin,
        tokio::spawn(axum::serve(listener, app).into_future()),
    )
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
            .route("/api/apps/work-orders/notifications", post(publish))
            .route("/redirect-trap", any(trap))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let platform_task = tokio::spawn(axum::serve(listener, platform).into_future());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let platform_origin = format!("http://{}", listener.local_addr().unwrap());
        let proxy_state = state.clone();
        let proxy = tokio::spawn(async move {
            let mut children = JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{let (socket,_)=accepted.unwrap();let upstream=upstream.clone();let state=proxy_state.clone();children.spawn(async move{tokio::time::timeout(Duration::from_secs(30),proxy_connection(socket,upstream,state)).await.expect("fixture proxy deadline");});},
                    completed=children.join_next(),if !children.is_empty()=>{completed.unwrap().expect("fixture proxy task");}
                }
            }
        });
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
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
        for (key, value) in overrides {
            pairs.insert((*key).to_owned(), (*value).to_owned());
        }
        let config = Config::from_pairs(pairs).unwrap();
        let pool = connect_database(&config).await.unwrap();
        f.pools.lock().unwrap().push(pool.clone());
        let (origin, business_task) = serve_business(&config, pool.clone()).await;
        let h = Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(25))
                .build()
                .unwrap(),
            origin,
            pool,
            config,
            state,
            tasks: vec![platform_task, business_task],
            proxy,
        };
        h.put("actor", Scenario::allowed(f.identity));
        h
    }
    async fn restart(&mut self, f: &Fixture) {
        self.tasks.pop().unwrap().abort();
        self.pool.close().await;
        self.pool = connect_database(&self.config).await.unwrap();
        f.pools.lock().unwrap().push(self.pool.clone());
        let (origin, task) = serve_business(&self.config, self.pool.clone()).await;
        self.origin = origin;
        self.tasks.push(task);
    }
    fn put(&self, token: &str, scenario: Scenario) {
        self.state
            .lock()
            .unwrap()
            .scenarios
            .insert(token.to_owned(), scenario);
    }
    fn calls(&self) -> Vec<PublicationCall> {
        self.state.lock().unwrap().calls.clone()
    }
    fn publication_count(&self) -> usize {
        self.state.lock().unwrap().publications.len()
    }
    async fn raw(
        &self,
        method: Method,
        token: &str,
        path: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Reply {
        let mut request = self
            .client
            .request(method, format!("{}{PREFIX}{path}", self.origin))
            .header("origin", "null")
            .bearer_auth(token)
            .body(body);
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        let response = request.send().await.expect("business reminder response");
        Reply {
            status: response.status().as_u16(),
            headers: response.headers().clone(),
            body: response.bytes().await.unwrap().to_vec(),
        }
    }
    async fn remind(&self, token: &str, id: Uuid, key: &str) -> Reply {
        self.raw(
            Method::POST,
            token,
            &format!("/tickets/{id}/remind"),
            serde_json::to_vec(&json!({"idempotencyKey":key})).unwrap(),
            Some("application/json"),
        )
        .await
    }
    fn spawn_reminder(&self, token: &str, id: Uuid, key: &str) -> JoinHandle<Reply> {
        let client = self.client.clone();
        let origin = self.origin.clone();
        let token = token.to_owned();
        let key = key.to_owned();
        tokio::spawn(async move {
            let response = client
                .post(format!("{origin}{PREFIX}/tickets/{id}/remind"))
                .header("origin", "null")
                .bearer_auth(token)
                .json(&json!({"idempotencyKey":key}))
                .send()
                .await
                .unwrap();
            Reply {
                status: response.status().as_u16(),
                headers: response.headers().clone(),
                body: response.bytes().await.unwrap().to_vec(),
            }
        })
    }
}

async fn ticket(f: &Fixture, identity: Identity, assignee: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    let number = format!("WO-PH20-{}", id.simple());
    sqlx::query("INSERT INTO public.work_orders(id,number,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,assignee_user_id,assignee_display_name,title) VALUES($1,$2,$3,$4,'提醒组织',$5,'历史owner',$6,$7,'Editable title')")
        .bind(id).bind(number).bind(identity.tenant).bind(identity.org).bind(identity.user).bind(assignee).bind(assignee.map(|_|"历史处理人")).execute(&f.pool).await.unwrap();
    id
}

async fn row(f: &Fixture, id: Uuid) -> String {
    sqlx::query_scalar("SELECT row_to_json(t)::text FROM public.work_orders t WHERE id=$1")
        .bind(id)
        .fetch_one(&f.pool)
        .await
        .unwrap()
}

async fn assign(f: &Fixture, id: Uuid, assignee: Uuid) {
    sqlx::query("UPDATE public.work_orders SET assignee_user_id=$2,assignee_display_name='更改处理人',version=version+1,updated_at=statement_timestamp() WHERE id=$1")
        .bind(id).bind(assignee).execute(&f.pool).await.unwrap();
}

fn expected_payload(id: Uuid, identity: Identity, recipient: Uuid, key: &str) -> Value {
    json!({"idempotencyKey":key,"target":{"kind":"USER","userId":recipient.to_string(),"organizationId":identity.org.to_string()},
        "type":"INFO","title":format!("工单提醒 WO-PH20-{}",id.simple()),"content":format!("请查看工单 WO-PH20-{} 的详情并处理。",id.simple()),
        "link":format!("/micro/work-orders/tickets/{id}")})
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn stable_event_payload_single_publication_and_unchanged_ticket() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let id = ticket(&f, f.identity, None).await;
        let before = row(&f, id).await;
        let first = h.remind("actor", id, "stable-event").await;
        assert_eq!(first.status, 200, "{}", first.text());
        assert_eq!(first.json()["duplicate"], false);
        let retry = h.remind("actor", id, "stable-event").await;
        assert_eq!(retry.status, 200, "{}", retry.text());
        assert_eq!(retry.json()["duplicate"], true);
        for field in [
            "publicationId",
            "recipientCount",
            "deliveryCount",
            "createdAt",
        ] {
            assert_eq!(retry.json()[field], first.json()[field]);
        }
        let mut keys: Vec<_> = first.json().as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "createdAt",
                "deliveryCount",
                "duplicate",
                "publicationId",
                "recipientCount"
            ]
        );
        assert_eq!(h.publication_count(), 1);
        assert_eq!(h.calls().len(), 2);
        assert_eq!(row(&f, id).await, before);
        for call in h.calls() {
            assert_eq!(call.token, "actor");
            assert!(Uuid::parse_str(&call.request_id).is_ok());
            assert_eq!(
                call.payload,
                expected_payload(id, f.identity, f.identity.user, "stable-event")
            );
        }
        let state = h.state.lock().unwrap();
        assert_eq!(state.introspections, 2);
        assert_eq!(state.contexts, 2);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn recipient_selection_and_title_edits_preserve_event_body() {
    owned(|f|async move{
        let h=Harness::new(&f).await;let recipient=Uuid::new_v4();let id=ticket(&f,f.identity,Some(recipient)).await;
        let first=h.remind("actor",id,"assigned-event").await;assert_eq!(first.status,200);
        assert_eq!(h.calls()[0].payload,expected_payload(id,f.identity,recipient,"assigned-event"));
        sqlx::query("UPDATE public.work_orders SET title='Changed display title',description='Changed',version=version+1,updated_at=statement_timestamp() WHERE id=$1").bind(id).execute(&f.pool).await.unwrap();
        let before=row(&f,id).await;let retry=h.remind("actor",id,"assigned-event").await;assert_eq!(retry.status,200);assert_eq!(retry.json()["duplicate"],true);
        assert_eq!(h.calls()[0].payload,h.calls()[1].payload);assert_eq!(row(&f,id).await,before);
        // A historical receipt is not a claim that the recipient is still active;
        // no directory preflight should block the official platform's receipt.
        assert_eq!(h.publication_count(),1);
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn ticket_and_notification_authorizations_are_independent() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let id = ticket(&f, f.identity, None).await;
        let before = row(&f, id).await;
        for (case, expected) in [
            (0, "OPERATION_DENIED"),
            (1, "DATA_DENIED"),
            (2, "OPERATION_DENIED"),
            (3, "DATA_DENIED"),
        ] {
            let mut scenario = Scenario::allowed(f.identity);
            match case {
                0 => scenario.permissions.retain(|p| p != "ticket:read"),
                1 => scenario.ticket_grant = false,
                2 => scenario.permissions.retain(|p| p != "notification:publish"),
                _ => scenario.notification_grant = false,
            }
            h.put("denied", scenario);
            h.remind("denied", id, &format!("denied-{case}"))
                .await
                .error(403, expected);
        }
        let mut duplicate = Scenario::allowed(f.identity);
        let mut context = duplicate.context();
        let grant = context["dataGrants"][1].clone();
        context["dataGrants"].as_array_mut().unwrap().push(grant);
        duplicate.context_patch = json!({"dataGrants":context["dataGrants"]});
        h.put("duplicate", duplicate);
        h.remind("duplicate", id, "duplicate-notify-grant")
            .await
            .error(403, "DATA_DENIED");
        assert!(h.calls().is_empty());
        assert_eq!(h.publication_count(), 0);
        assert_eq!(row(&f, id).await, before);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn notification_scope_and_ticket_visibility_enforce_organization_ceiling() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let child = Uuid::new_v4();
        let sibling = Uuid::new_v4();
        let other = Uuid::new_v4();
        let own = ticket(&f, f.identity, None).await;
        let assigned = ticket(&f, f.identity, Some(other)).await;
        let child_id = ticket(
            &f,
            Identity {
                org: child,
                ..f.identity
            },
            None,
        )
        .await;
        let sibling_id = ticket(
            &f,
            Identity {
                org: sibling,
                ..f.identity
            },
            None,
        )
        .await;
        let foreign = ticket(
            &f,
            Identity {
                tenant: Uuid::new_v4(),
                ..f.identity
            },
            None,
        )
        .await;
        let hidden = ticket(
            &f,
            Identity {
                user: other,
                ..f.identity
            },
            Some(f.identity.user),
        )
        .await;
        let deleted = ticket(&f, f.identity, None).await;
        sqlx::query("UPDATE public.work_orders SET deleted_at=now() WHERE id=$1")
            .bind(deleted)
            .execute(&f.pool)
            .await
            .unwrap();
        let mut scenario = Scenario::allowed(f.identity);
        scenario.notification_scope = "SELF";
        h.put("scoped", scenario.clone());
        assert_eq!(h.remind("scoped", own, "self-own").await.status, 200);
        h.remind("scoped", assigned, "self-other")
            .await
            .error(403, "DATA_DENIED");
        h.remind("scoped", child_id, "self-child")
            .await
            .error(403, "DATA_DENIED");
        scenario.notification_scope = "ORG";
        scenario.notification_orgs = vec![f.identity.org];
        h.put("scoped", scenario.clone());
        assert_eq!(h.remind("scoped", assigned, "org-other").await.status, 200);
        h.remind("scoped", child_id, "org-child")
            .await
            .error(403, "DATA_DENIED");
        scenario.notification_scope = "ORG_SUBTREE";
        scenario.notification_orgs = vec![f.identity.org, child];
        h.put("scoped", scenario.clone());
        assert_eq!(
            h.remind("scoped", child_id, "subtree-child").await.status,
            200
        );
        h.remind("scoped", sibling_id, "subtree-sibling")
            .await
            .error(403, "DATA_DENIED");
        scenario.notification_scope = "TENANT";
        scenario.notification_orgs = vec![];
        scenario.ceiling = json!([f.identity.org.to_string(), child.to_string()]);
        h.put("scoped", scenario.clone());
        assert_eq!(
            h.remind("scoped", child_id, "tenant-within-ceiling")
                .await
                .status,
            200
        );
        h.remind("scoped", sibling_id, "ceiling-sibling")
            .await
            .error(404, "RESOURCE_NOT_FOUND");
        scenario.ceiling = Value::Null;
        scenario.ticket_scope = "SELF";
        h.put("scoped", scenario);
        h.remind("scoped", hidden, "assignee-not-owner")
            .await
            .error(404, "RESOURCE_NOT_FOUND");
        for id in [foreign, deleted, Uuid::new_v4()] {
            h.remind("actor", id, "invisible")
                .await
                .error(404, "RESOURCE_NOT_FOUND");
        }
        assert_eq!(h.calls().len(), 4);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn reminder_input_is_strict_and_keys_are_body_events() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let id = ticket(&f, f.identity, None).await;
        let path = format!("/tickets/{id}/remind");
        for body in [
            json!({}),
            json!(null),
            json!([]),
            json!({"idempotencyKey":null}),
            json!({"idempotencyKey":1}),
            json!({"idempotencyKey":""}),
            json!({"idempotencyKey":"bad key"}),
            json!({"idempotencyKey":"界"}),
            json!({"idempotencyKey":"x".repeat(129)}),
            json!({"idempotencyKey":"forged","target":{"kind":"TENANT"}}),
            json!({"idempotencyKey":"forged","title":"changed"}),
            json!({"idempotencyKey":"forged","version":1}),
        ] {
            h.raw(
                Method::POST,
                "actor",
                &path,
                serde_json::to_vec(&body).unwrap(),
                Some("application/json"),
            )
            .await
            .error(400, "VALIDATION_ERROR");
        }
        for body in [
            r#"{"idempotencyKey":"a","idempotencyKey":"b"}"#,
            "{invalid",
            r#"{"idempotencyKey":"a"} trailing"#,
        ] {
            h.raw(
                Method::POST,
                "actor",
                &path,
                body.as_bytes().to_vec(),
                Some("application/json"),
            )
            .await
            .error(400, "VALIDATION_ERROR");
        }
        for content_type in [None, Some("text/plain"), Some("text/html")] {
            h.raw(
                Method::POST,
                "actor",
                &path,
                br#"{"idempotencyKey":"valid"}"#.to_vec(),
                content_type,
            )
            .await
            .error(400, "VALIDATION_ERROR");
        }
        for invalid in ["bad-uuid", &id.simple().to_string()] {
            h.raw(
                Method::POST,
                "actor",
                &format!("/tickets/{invalid}/remind"),
                br#"{"idempotencyKey":"valid"}"#.to_vec(),
                Some("application/json"),
            )
            .await
            .error(400, "VALIDATION_ERROR");
        }
        h.raw(
            Method::POST,
            "actor",
            &format!("{path}?extra=x"),
            br#"{"idempotencyKey":"valid"}"#.to_vec(),
            Some("application/json"),
        )
        .await
        .error(400, "VALIDATION_ERROR");
        h.raw(
            Method::POST,
            "actor",
            &path,
            vec![b'x'; 65537],
            Some("application/json"),
        )
        .await
        .error(413, "REQUEST_LIMIT");
        let method = h.raw(Method::GET, "actor", &path, vec![], None).await;
        assert_eq!(method.status, 405);
        assert!(h.calls().is_empty());
        for key in ["a".to_owned(), "aZ09._:-".repeat(16)] {
            assert_eq!(key.len().min(128), key.len());
            assert_eq!(h.remind("actor", id, &key).await.status, 200);
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn upstream_failures_do_not_retry_or_leak_and_preserve_ticket() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let id = ticket(&f, f.identity, None).await;
        let before = row(&f, id).await;
        for (index, status) in [400, 401, 403, 404, 409, 429, 500, 503, 201, 202, 204, 302]
            .into_iter()
            .enumerate()
        {
            let mut scenario = Scenario::allowed(f.identity);
            scenario.mode = PublishMode::Status(status);
            h.put("failure", scenario);
            let (expected, code) = match status {
                400 => (400, "VALIDATION_ERROR"),
                401 => (401, "INVALID_APP_TOKEN"),
                403 => (403, "DATA_DENIED"),
                404 => (404, "RECIPIENT_NOT_FOUND"),
                409 => (409, "IDEMPOTENCY_CONFLICT"),
                _ => (503, "REMINDER_RESULT_UNKNOWN"),
            };
            let calls = h.calls().len();
            h.remind("failure", id, &format!("status-{index}"))
                .await
                .error(expected, code);
            assert_eq!(h.calls().len(), calls + 1, "no status-triggered retry");
        }
        assert_eq!(h.publication_count(), 0);
        assert_eq!(row(&f, id).await, before);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn post_persistence_response_loss_and_timeout_recover_same_event() {
    owned(|f|async move{
        let h=Harness::configured(&f,&[("PLATFORM_AUTH_TIMEOUT_MS","500")]).await;let id=ticket(&f,f.identity,None).await;let before=row(&f,id).await;
        let mut scenario=Scenario::allowed(f.identity);scenario.mode=PublishMode::DropAfterPersist;h.put("drop",scenario);
        h.remind("drop",id,"lost-event").await.error(503,"REMINDER_RESULT_UNKNOWN");assert_eq!(h.calls().len(),1);assert_eq!(h.publication_count(),1);
        let original=h.calls()[0].payload.clone();h.put("drop",Scenario::allowed(f.identity));let retry=h.remind("drop",id,"lost-event").await;
        assert_eq!(retry.status,200);assert_eq!(retry.json()["duplicate"],true);assert_eq!(h.calls()[1].payload,original);assert_eq!(h.publication_count(),1);
        println!("WORK_ORDERS_REMINDER_UNKNOWN_RESULT_PROOF persisted=true same_key=true unchanged_payload=true publication_count=1 no_automatic_retry=true");
        let gate=Gate::new();let mut scenario=Scenario::allowed(f.identity);scenario.mode=PublishMode::GateAfterPersist(gate.clone());h.put("timeout",scenario);
        let pending=h.spawn_reminder("timeout",id,"timeout-event");gate.observed().await;let timed=pending.await.unwrap();timed.error(503,"REMINDER_RESULT_UNKNOWN");assert_eq!(h.calls().len(),3);assert_eq!(h.publication_count(),2);
        gate.release.add_permits(1);h.put("timeout",Scenario::allowed(f.identity));let restored=h.remind("timeout",id,"timeout-event").await;
        assert_eq!(restored.status,200);assert_eq!(restored.json()["duplicate"],true);assert_eq!(h.calls()[2].payload,h.calls()[3].payload);assert_eq!(h.publication_count(),2);assert_eq!(row(&f,id).await,before);
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn durable_intent_survives_restart_and_recipient_or_ticket_changes_conflict() {
    owned(|f|async move{
        let mut h=Harness::new(&f).await;let id=ticket(&f,f.identity,None).await;
        let mut scenario=Scenario::allowed(f.identity);scenario.mode=PublishMode::Status(503);h.put("actor",scenario.clone());
        h.remind("actor",id,"restart-recovery").await.error(503,"REMINDER_RESULT_UNKNOWN");assert_eq!(h.publication_count(),0);let original=h.calls()[0].payload.clone();
        h.restart(&f).await;h.put("actor",Scenario::allowed(f.identity));let restored=h.remind("actor",id,"restart-recovery").await;assert_eq!(restored.status,200);assert_eq!(restored.json()["duplicate"],false);assert_eq!(h.calls()[1].payload,original);
        h.put("actor",scenario);h.remind("actor",id,"uncommitted-event").await.error(503,"REMINDER_RESULT_UNKNOWN");assert_eq!(h.publication_count(),1);
        h.restart(&f).await;let recipient=Uuid::new_v4();assign(&f,id,recipient).await;h.put("actor",Scenario::allowed(f.identity));let calls=h.calls().len();
        h.remind("actor",id,"uncommitted-event").await.error(409,"IDEMPOTENCY_CONFLICT");assert_eq!(h.calls().len(),calls,"changed target rejected before platform even when no platform commit existed");
        h.remind("actor",id,"restart-recovery").await.error(409,"IDEMPOTENCY_CONFLICT");assert_eq!(h.calls().len(),calls);
        let fresh=h.remind("actor",id,"explicit-new-event").await;assert_eq!(fresh.status,200);assert_eq!(h.calls().last().unwrap().payload,expected_payload(id,f.identity,recipient,"explicit-new-event"));
        let another=ticket(&f,f.identity,Some(recipient)).await;let calls=h.calls().len();h.remind("actor",another,"explicit-new-event").await.error(409,"IDEMPOTENCY_CONFLICT");assert_eq!(h.calls().len(),calls);
        println!("WORK_ORDERS_REMINDER_RECIPIENT_CHANGE_PROOF same_key_conflict=true explicit_new_event=true uncommitted_intent_preserved=true application_restarted=true");
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn successful_publication_response_is_bounded_validated_and_projected() {
    owned(|f|async move{
        let h=Harness::new(&f).await;let id=ticket(&f,f.identity,None).await;
        let patches=vec![json!({"publicationId":"invalid"}),json!({"publicationId":Uuid::new_v4().simple().to_string()}),json!({"recipientCount":0}),json!({"recipientCount":2}),json!({"recipientCount":1.5}),json!({"recipientCount":"1"}),json!({"recipientCount":1.0}),json!({"deliveryCount":0}),json!({"deliveryCount":2}),json!({"deliveryCount":"1"}),json!({"deliveryCount":1.0}),json!({"duplicate":1}),json!({"duplicate":null}),json!({"createdAt":"yesterday"}),json!({"createdAt":"2026-10-17"}),json!({"createdAt":0})];
        let mut modes:Vec<_>=patches.into_iter().map(PublishMode::Patch).collect();
        for missing in ["publicationId", "recipientCount", "deliveryCount", "duplicate", "createdAt"] {
            let mut body = json!({"publicationId":Uuid::new_v4().to_string(),"recipientCount":1,"deliveryCount":1,"duplicate":false,"createdAt":"2026-10-17T00:00:00Z"});
            body.as_object_mut().unwrap().remove(missing);
            modes.push(PublishMode::Raw(body.to_string()));
        }
        modes.extend([PublishMode::Raw("{bad json".into()),PublishMode::Raw("[]".into()),PublishMode::Raw("null".into()),PublishMode::Raw(r#"{"publicationId":"33333333-3333-4333-8333-333333333333","recipientCount":1,"deliveryCount":1,"duplicate":false,"duplicate":true,"createdAt":"2026-10-17T00:00:00Z"}"#.into()),PublishMode::Html,PublishMode::Oversize,PublishMode::Redirect]);
        for (index,mode) in modes.into_iter().enumerate(){let mut scenario=Scenario::allowed(f.identity);scenario.mode=mode;h.put("response",scenario);let before=h.calls().len();h.remind("response",id,&format!("invalid-response-{index}")).await.error(503,"REMINDER_RESULT_UNKNOWN");assert_eq!(h.calls().len(),before+1);}
        assert_eq!(h.state.lock().unwrap().redirect_hits,0);
        let mut scenario=Scenario::allowed(f.identity);scenario.mode=PublishMode::Patch(json!({"online":false,"socketReady":false,"padding":"untrusted"}));h.put("response",scenario);
        let valid=h.remind("response",id,"valid-projected").await;assert_eq!(valid.status,200,"{}",valid.text());assert_eq!(valid.json().as_object().unwrap().len(),5);assert!(valid.json().get("online").is_none());
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn publication_http_releases_the_business_pool_and_ticket_row_lock() {
    owned(|f|async move{
        let h=Harness::configured(&f,&[("WORK_ORDERS_DB_MAX_CONNECTIONS","1")]).await;let id=ticket(&f,f.identity,None).await;
        let gate=Gate::new();let mut scenario=Scenario::allowed(f.identity);scenario.mode=PublishMode::GateAfterPersist(gate.clone());h.put("actor",scenario);
        let pending=h.spawn_reminder("actor",id,"pool-release");gate.observed().await;assert!(!pending.is_finished());
        let mut connection=tokio::time::timeout(Duration::from_millis(500),h.pool.acquire()).await.expect("sole business connection released before platform HTTP").unwrap();
        let one:i32=sqlx::query_scalar("SELECT 1").fetch_one(&mut *connection).await.unwrap();assert_eq!(one,1);drop(connection);
        let mut transaction=f.pool.begin().await.unwrap();let locked:Uuid=sqlx::query_scalar("SELECT id FROM public.work_orders WHERE id=$1 FOR UPDATE NOWAIT").bind(id).fetch_one(&mut *transaction).await.expect("no ticket row lock during platform HTTP");assert_eq!(locked,id);
        sqlx::query("UPDATE public.work_orders SET title='Concurrent edit while HTTP pending',version=version+1,updated_at=statement_timestamp() WHERE id=$1").bind(id).execute(&mut *transaction).await.unwrap();transaction.commit().await.unwrap();
        let after_edit=row(&f,id).await;assert!(!pending.is_finished());
        gate.release.add_permits(1);let reply=pending.await.unwrap();assert_eq!(reply.status,200,"{}",reply.text());assert_eq!(row(&f,id).await,after_edit);
        println!("WORK_ORDERS_REMINDER_DB_RELEASE_PROOF pool_available=true row_unlocked=true publication_pending=true");
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn authorization_revocation_and_context_expiry_cannot_replay_cached_success() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let id = ticket(&f, f.identity, None).await;
        assert_eq!(h.remind("actor", id, "original-success").await.status, 200);
        let calls = h.calls().len();
        let mut scenario = Scenario::allowed(f.identity);
        scenario.active = false;
        h.put("actor", scenario);
        h.remind("actor", id, "original-success")
            .await
            .error(401, "INVALID_APP_TOKEN");
        assert_eq!(h.calls().len(), calls);
        let mut scenario = Scenario::allowed(f.identity);
        scenario.notification_grant = false;
        h.put("actor", scenario);
        h.remind("actor", id, "original-success")
            .await
            .error(403, "DATA_DENIED");
        assert_eq!(h.calls().len(), calls);
        let mut scenario = Scenario::allowed(f.identity);
        scenario.context_patch =
            json!({"user":{"id":Uuid::new_v4().to_string(),"username":"wrong"}});
        h.put("actor", scenario);
        h.remind("actor", id, "original-success")
            .await
            .error(401, "INVALID_CONTEXT");
        assert_eq!(h.calls().len(), calls);
        let mut scenario = Scenario::allowed(f.identity);
        scenario.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        h.put("actor", scenario);
        h.remind("actor", id, "original-success")
            .await
            .error(401, "INVALID_CONTEXT");
        assert_eq!(h.calls().len(), calls);
        h.put("actor", Scenario::allowed(f.identity));
        assert_eq!(
            h.remind("actor", id, "original-success").await.json()["duplicate"],
            true
        );
        let gate = Gate::new();
        let mut scenario = Scenario::allowed(f.identity);
        scenario.expires_at = Some(Utc::now() + chrono::Duration::milliseconds(700));
        scenario.mode = PublishMode::GateAfterPersist(gate.clone());
        h.put("expires", scenario);
        let pending = h.spawn_reminder("expires", id, "expires-after-publish");
        gate.observed().await;
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        let expiry = h.state.lock().unwrap().scenarios["expires"]
            .expires_at
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while Utc::now() <= expiry {
                interval.tick().await;
            }
        })
        .await
        .unwrap();
        gate.release.add_permits(1);
        pending.await.unwrap().error(503, "REMINDER_RESULT_UNKNOWN");
        h.put("expires", Scenario::allowed(f.identity));
        let restored = h.remind("expires", id, "expires-after-publish").await;
        assert_eq!(restored.status, 200);
        assert_eq!(restored.json()["duplicate"], true);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn preparation_commit_failure_and_concurrent_event_conflicts_fail_closed() {
    owned(|f| async move {
        let h = Harness::configured(&f, &[("PLATFORM_AUTH_MAX_CONCURRENT", "1")]).await;
        let id = ticket(&f, f.identity, None).await;
        let before = row(&f, id).await;
        f.pool.execute("CREATE FUNCTION public.phase20_reject_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned deferred commit rejection'; END $$; CREATE CONSTRAINT TRIGGER phase20_commit_rejection AFTER INSERT ON public.work_order_reminder_intents DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.phase20_reject_commit()").await.unwrap();
        h.remind("actor", id, "prepare-failure")
            .await
            .error(503, "REMINDER_PREPARATION_UNKNOWN");
        assert!(h.calls().is_empty());
        assert_eq!(row(&f, id).await, before);
        f.pool.execute("DROP TRIGGER phase20_commit_rejection ON public.work_order_reminder_intents; DROP FUNCTION public.phase20_reject_commit()").await.unwrap();
        println!("WORK_ORDERS_REMINDER_PREPARATION_PROOF commit_rejected=true publication_posts=0 ticket_unchanged=true");
        let gate = Gate::new();
        let mut scenario = Scenario::allowed(f.identity);
        scenario.mode = PublishMode::GateAfterPersist(gate.clone());
        h.put("actor", scenario);
        let pending = h.spawn_reminder("actor", id, "concurrent-event");
        gate.observed().await;
        let started = Instant::now();
        h.remind("actor", id, "other-during-capacity")
            .await
            .error(503, "PLATFORM_UNAVAILABLE");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(h.calls().len(), 1);
        gate.release.add_permits(1);
        assert_eq!(pending.await.unwrap().status, 200);
        h.put("actor", Scenario::allowed(f.identity));

        // Corruption is deliberately injected only in this owned database. The
        // immutable trigger is restored before exercising the public HTTP API.
        assert_eq!(h.remind("actor", id, "corrupt-event").await.status, 200);
        f.pool.execute("ALTER TABLE public.work_order_reminder_intents DISABLE TRIGGER work_order_reminder_intent_immutable; UPDATE public.work_order_reminder_intents SET request_hash=repeat('0',64) WHERE idempotency_key='corrupt-event'; ALTER TABLE public.work_order_reminder_intents ENABLE TRIGGER work_order_reminder_intent_immutable").await.unwrap();
        let calls = h.calls().len();
        h.remind("actor", id, "corrupt-event")
            .await
            .error(503, "DATABASE_UNAVAILABLE");
        assert_eq!(h.calls().len(), calls, "damaged intent never reaches platform HTTP");

        // A separate application has ordinary capacity, so both requests reach
        // local intent arbitration rather than merely saturating authorization.
        let race = Harness::new(&f).await;
        let other = ticket(&f, f.identity, None).await;
        let first = race.spawn_reminder("actor", id, "same-key-different-ticket");
        let second = race.spawn_reminder("actor", other, "same-key-different-ticket");
        let first = first.await.unwrap();
        let second = second.await.unwrap();
        let mut statuses = [first.status, second.status];
        statuses.sort();
        assert_eq!(statuses, [200, 409], "one concurrent event wins: {first:?} {second:?}");
        if first.status == 409 { first.error(409, "IDEMPOTENCY_CONFLICT"); }
        if second.status == 409 { second.error(409, "IDEMPOTENCY_CONFLICT"); }
        assert_eq!(race.calls().len(), 1);
        assert_eq!(race.publication_count(), 1);

        let same_a = race.spawn_reminder("actor", id, "same-key-same-ticket");
        let same_b = race.spawn_reminder("actor", id, "same-key-same-ticket");
        let a = same_a.await.unwrap();
        let b = same_b.await.unwrap();
        assert_eq!(a.status, 200);
        assert_eq!(b.status, 200);
        assert_eq!(a.json()["publicationId"], b.json()["publicationId"]);
        assert_ne!(a.json()["duplicate"], b.json()["duplicate"]);
        assert_eq!(race.publication_count(), 2);
        assert_eq!(race.calls().len(), 3);
        assert_eq!(row(&f, id).await, before);
    })
    .await;
}
