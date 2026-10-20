//! Explicit, owned PostgreSQL + controlled platform HTTP fixture regressions.
//! These tests never use a developer's configured business or platform database.
use std::{
    collections::{BTreeSet, HashMap},
    future::{Future, IntoFuture},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Router,
};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, Executor, PgPool, Postgres, QueryBuilder};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;
use work_orders_api::{application, initialize_database, Config};

const PREFIX: &str = "/work-orders-api";
const SECRET: &str = "phase17-fixture-secret";
const BASIC: &str = "Basic aW50ZWdyYXRpb246cGhhc2UxNy1maXh0dXJlLXNlY3JldA==";

#[derive(Clone)]
struct Fixture {
    pool: PgPool,
    database_url: String,
    tenant: Uuid,
    other_tenant: Uuid,
    user: Uuid,
    other_user: Uuid,
    org: Uuid,
    child: Uuid,
    parent: Uuid,
    peer: Uuid,
    tickets: Vec<Uuid>,
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
        .expect("owned admin connection");
    let name = format!("work_orders_phase17_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&admin)
        .await
        .expect("create owned database");
    url.set_path(&format!("/{name}"));
    let result = PgPoolOptions::new().max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .after_connect(|connection, _| Box::pin(async move {
            connection.execute("SET statement_timeout='15s'; SET lock_timeout='5s'; SET search_path=pg_catalog,public,pg_temp").await?;
            Ok(())
        }))
        .connect(url.as_str()).await;
    let pool = match result {
        Ok(pool) => pool,
        Err(_) => {
            let removed = sqlx::query(&format!("DROP DATABASE \"{name}\" WITH (FORCE)"))
                .execute(&admin)
                .await
                .is_ok();
            admin.close().await;
            println!(
                "WORK_ORDERS_FIXTURE_SETUP_CLEANUP database_removed={removed} pool_closed=true"
            );
            assert!(removed, "owned setup cleanup failed");
            panic!("owned business pool setup failed");
        }
    };
    // The CREATE above succeeded before the name can reach this cleanup path.
    let fixture = Fixture {
        pool: pool.clone(),
        database_url: url.to_string(),
        tenant: Uuid::new_v4(),
        other_tenant: Uuid::new_v4(),
        user: Uuid::new_v4(),
        other_user: Uuid::new_v4(),
        org: Uuid::new_v4(),
        child: Uuid::new_v4(),
        parent: Uuid::new_v4(),
        peer: Uuid::new_v4(),
        tickets: (0..8).map(|_| Uuid::new_v4()).collect(),
    };
    let result = tokio::spawn(async move {
        initialize_database(&fixture.pool)
            .await
            .expect("business initialization");
        seed(&fixture).await;
        case(fixture).await;
    })
    .await;
    pool.close().await;
    let removed = sqlx::query(&format!("DROP DATABASE \"{name}\" WITH (FORCE)"))
        .execute(&admin)
        .await
        .is_ok();
    admin.close().await;
    println!(
        "WORK_ORDERS_FIXTURE_CLEANUP assertions_ok={} database_removed={} pool_closed=true",
        result.is_ok(),
        removed
    );
    assert!(removed, "owned database cleanup failed");
    if let Err(error) = result {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("owned test task cancelled");
    }
}

async fn seed(f: &Fixture) {
    let identities = [
        (f.tenant, f.org, f.user),
        (f.tenant, f.org, f.other_user),
        (f.tenant, f.child, f.user),
        (f.tenant, f.child, f.other_user),
        (f.tenant, f.parent, f.user),
        (f.tenant, f.peer, f.user),
        (f.other_tenant, f.org, f.user),
        (f.tenant, f.org, f.user),
    ];
    for (i, (tenant, org, owner)) in identities.into_iter().enumerate() {
        sqlx::query("INSERT INTO work_orders(id,number,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,title,description,priority,status,created_at,updated_at,deleted_at) VALUES($1,$2,$3,$4,'测试组织',$5,'测试创建人',$6,$7,$8,$9,'2026-01-02T03:04:05.123Z','2026-01-02T03:04:05.123Z',CASE WHEN $10 THEN now() ELSE NULL END)")
            .bind(f.tickets[i]).bind(format!("WO-{i:06}")).bind(tenant).bind(org).bind(owner)
            .bind(if i == 0 { "Literal 100%_\\ path 中文" } else { "ordinary ticket" })
            .bind(if i == 0 { "Literal match only" } else { "ordinary description" })
            .bind(if i == 0 { "HIGH" } else { "NORMAL" })
            .bind(if i == 0 { "IN_PROGRESS" } else { "OPEN" }).bind(i == 7)
            .execute(&f.pool).await.expect("owned ticket seed");
    }
}

#[derive(Clone, Default)]
enum Mode {
    #[default]
    Json,
    Delay,
    Oversize,
    Malformed,
    Html,
    Redirect,
}

#[derive(Clone)]
struct Scenario {
    intro: Value,
    context: Value,
    intro_status: StatusCode,
    context_status: StatusCode,
    intro_mode: Mode,
    context_mode: Mode,
}

#[derive(Clone)]
struct Call {
    stage: &'static str,
    token: String,
    authorization: String,
    content_type: String,
    request_id: String,
}

#[derive(Default)]
struct PlatformState {
    scenarios: HashMap<String, Scenario>,
    calls: Vec<Call>,
    redirect_hits: usize,
}

type SharedPlatform = Arc<Mutex<PlatformState>>;

fn response(status: StatusCode, mode: &Mode, value: Value) -> Response {
    match mode {
        Mode::Oversize => {
            let mut padded = value;
            padded["padding"] = json!("x".repeat(300_000));
            (status, axum::Json(padded)).into_response()
        }
        Mode::Malformed => (
            status,
            [("content-type", "application/json")],
            format!("{{invalid-json {SECRET}"),
        )
            .into_response(),
        Mode::Html => (
            status,
            [("content-type", "text/html")],
            format!("<html>platform detail must remain private {SECRET}</html>"),
        )
            .into_response(),
        Mode::Redirect => (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "/redirect-target"), ("connection", "close")],
            Body::empty(),
        )
            .into_response(),
        Mode::Json | Mode::Delay => (status, axum::Json(value)).into_response(),
    }
}

async fn introspect(State(shared): State<SharedPlatform>, request: Request) -> Response {
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), 16_384)
        .await
        .expect("bounded introspection request");
    let fields: Vec<_> = url::form_urlencoded::parse(&body).into_owned().collect();
    assert_eq!(fields.len(), 1, "introspection sends only the token");
    assert_eq!(fields[0].0, "token");
    let token = &fields[0].1;
    let scenario = {
        let mut state = shared.lock().expect("fixture state");
        state.calls.push(call("intro", token, &headers));
        state.scenarios.get(token).cloned()
    };
    let Some(scenario) = scenario else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if matches!(scenario.intro_mode, Mode::Delay) {
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    response(scenario.intro_status, &scenario.intro_mode, scenario.intro)
}

async fn context(State(shared): State<SharedPlatform>, request: Request) -> Response {
    assert!(
        request.uri().query().is_none(),
        "context contract has no query arguments"
    );
    let headers = request.headers();
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let scenario = {
        let mut state = shared.lock().expect("fixture state");
        state.calls.push(call("context", token, headers));
        state.scenarios.get(token).cloned()
    };
    let Some(scenario) = scenario else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if matches!(scenario.context_mode, Mode::Delay) {
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    response(
        scenario.context_status,
        &scenario.context_mode,
        scenario.context,
    )
}

async fn redirect_target(State(shared): State<SharedPlatform>) -> Response {
    shared.lock().expect("fixture state").redirect_hits += 1;
    axum::Json(json!({"active":false})).into_response()
}

fn call(stage: &'static str, token: &str, headers: &HeaderMap) -> Call {
    let value = |key| {
        headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    Call {
        stage,
        token: token.to_owned(),
        authorization: value("authorization"),
        content_type: value("content-type"),
        request_id: value("x-request-id"),
    }
}

struct Harness {
    client: reqwest::Client,
    origin: String,
    state: SharedPlatform,
    tasks: Vec<JoinHandle<std::io::Result<()>>>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

struct Reply {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("JSON API response")
    }
    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).expect("UTF-8 response")
    }
    fn error(&self, status: u16, code: &str) {
        assert_eq!(self.status, status, "{}", self.text());
        assert_eq!(self.json()["code"], code);
        assert!(!self.text().contains(SECRET));
        assert!(!self.text().contains(BASIC));
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
    }
}

impl Harness {
    async fn new(f: &Fixture) -> Self {
        let state = SharedPlatform::default();
        let platform = Router::new()
            .route("/oauth/introspect", post(introspect))
            .route("/api/apps/work-orders/context", get(context))
            .route("/redirect-target", any(redirect_target))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("platform fixture listener");
        let platform_origin = format!("http://{}", listener.local_addr().unwrap());
        let platform_task = tokio::spawn(axum::serve(listener, platform).into_future());
        let config = Config::from_pairs([
            ("WORK_ORDERS_DATABASE_URL", f.database_url.as_str()),
            ("PLATFORM_API_ORIGIN", platform_origin.as_str()),
            ("PLATFORM_CLIENT_ID", "integration"),
            ("PLATFORM_CLIENT_SECRET", SECRET),
            // This controlled fixture implements the original two-HTTP protocol.
            // Combined authorization is covered by the real-platform suites.
            ("PLATFORM_AUTHORIZATION_MODE", "legacy"),
            ("PLATFORM_AUTH_TIMEOUT_MS", "150"),
            ("WORK_ORDERS_ALLOWED_ORIGINS", "null"),
        ])
        .expect("isolated application config");
        assert!(!format!("{config:?}").contains(SECRET));
        let app = application(&config, f.pool.clone()).expect("protected application");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("business fixture listener");
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let business_task = tokio::spawn(axum::serve(listener, app).into_future());
        Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            origin,
            state,
            tasks: vec![platform_task, business_task],
        }
    }

    async fn scenario(
        &self,
        f: &Fixture,
        token: &str,
        scope: &str,
        organizations: Vec<Uuid>,
        ceiling: Value,
    ) -> Scenario {
        let expires: String = sqlx::query_scalar("SELECT to_char((clock_timestamp()+interval '25 seconds') AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')")
            .fetch_one(&f.pool).await.expect("fresh platform fixture expiry");
        let grant = |action| {
            json!({"resource":"ticket","action":action,"tenantId":f.tenant.to_string(),
            "subjectUserId":f.user.to_string(),"scope":scope,"organizationIds":organizations.iter().map(Uuid::to_string).collect::<Vec<_>>()})
        };
        let scenario = Scenario {
            intro: json!({"active":true,"aud":"work-orders","sub":f.user.to_string(),"tenant_id":f.tenant.to_string(),
                "organization_id":f.org.to_string(),"exp":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+600,"token_type":"Bearer"}),
            context: json!({"contractVersion":"1.0","appCode":"work-orders","contextVersion":"a".repeat(64),"expiresAt":expires,
                "user":{"id":f.user.to_string(),"username":"fixture","displayName":"测试用户"},
                "tenant":{"id":f.tenant.to_string()},"organization":{"id":f.org.to_string(),"name":"测试组织"},
                "application":{"id":Uuid::new_v4().to_string()},"organizationScopeIds":ceiling,
                "permissions":["ticket:read","ticket:export"],"dataGrants":[grant("read"),grant("export")]}),
            intro_status: StatusCode::OK,
            context_status: StatusCode::OK,
            intro_mode: Mode::Json,
            context_mode: Mode::Json,
        };
        self.put(token, scenario.clone());
        scenario
    }

    fn put(&self, token: &str, scenario: Scenario) {
        self.state
            .lock()
            .unwrap()
            .scenarios
            .insert(token.to_owned(), scenario);
    }

    async fn request(&self, token: Option<&str>, path: &str) -> Reply {
        let mut request = self
            .client
            .get(format!("{}{PREFIX}{path}", self.origin))
            .header("origin", "null");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.expect("business HTTP response");
        Reply {
            status: response.status().as_u16(),
            headers: response.headers().clone(),
            body: response.bytes().await.unwrap().to_vec(),
        }
    }

    fn assert_wire(&self) {
        let state = self.state.lock().unwrap();
        assert!(!state.calls.is_empty());
        for call in &state.calls {
            assert!(
                Uuid::parse_str(&call.request_id).is_ok(),
                "fresh trace propagated to each platform call"
            );
            if call.stage == "intro" {
                assert_eq!(call.authorization, BASIC);
                assert_eq!(call.content_type, "application/x-www-form-urlencoded");
            } else {
                assert_eq!(call.authorization, format!("Bearer {}", call.token));
                assert!(
                    state.calls.iter().any(|intro| intro.stage == "intro"
                        && intro.token == call.token
                        && intro.request_id == call.request_id),
                    "introspection and context share the request trace"
                );
            }
        }
    }
}

fn ids(value: &Value) -> BTreeSet<String> {
    value["records"]
        .as_array()
        .expect("list records")
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect()
}

fn expected(f: &Fixture, indexes: &[usize]) -> BTreeSet<String> {
    indexes.iter().map(|i| f.tickets[*i].to_string()).collect()
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

fn csv(reply: &Reply) -> Vec<Vec<String>> {
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(reply
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("text/csv"));
    assert!(
        reply.body.starts_with(&[0xef, 0xbb, 0xbf]),
        "CSV has UTF-8 BOM"
    );
    let chars: Vec<char> = reply
        .text()
        .trim_start_matches('\u{feff}')
        .chars()
        .collect();
    let (mut rows, mut row, mut cell) = (Vec::new(), Vec::new(), String::new());
    let (mut quoted, mut i) = (false, 0);
    while i < chars.len() {
        match chars[i] {
            '"' if quoted && chars.get(i + 1) == Some(&'"') => {
                cell.push('"');
                i += 1;
            }
            '"' => quoted = !quoted,
            ',' if !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {
                assert_eq!(chars.get(i + 1), Some(&'\n'));
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                i += 1;
            }
            ch => cell.push(ch),
        }
        i += 1;
    }
    assert!(
        !quoted && row.is_empty() && cell.is_empty(),
        "complete RFC4180 records"
    );
    assert!(rows.iter().all(|row| row.len() == 10));
    rows
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn scope_all_four_and_organization_ceiling() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let cases = [
            ("SELF", vec![], Value::Null, vec![0, 2, 4, 5]),
            ("SELF", vec![], jids(&[f.org, f.child]), vec![0, 2]),
            ("SELF", vec![f.org], Value::Null, vec![0]),
            ("ORG", vec![f.org], Value::Null, vec![0, 1]),
            (
                "ORG_SUBTREE",
                vec![f.org, f.child],
                Value::Null,
                vec![0, 1, 2, 3],
            ),
            (
                "ORG_SUBTREE",
                vec![f.org, f.child],
                jids(&[f.org]),
                vec![0, 1],
            ),
            ("TENANT", vec![], Value::Null, vec![0, 1, 2, 3, 4, 5]),
            ("TENANT", vec![], jids(&[f.org, f.child]), vec![0, 1, 2, 3]),
        ];
        for (index, (scope, orgs, ceiling, wanted)) in cases.into_iter().enumerate() {
            let token = format!("scope-{index}");
            h.scenario(&f, &token, scope, orgs, ceiling).await;
            let reply = h.request(Some(&token), "/tickets?size=100").await;
            assert_eq!(reply.status, 200, "{}", reply.text());
            assert_eq!(ids(&reply.json()), expected(&f, &wanted), "{scope}");
            assert_eq!(reply.json()["total"], wanted.len());
            for (index, id) in f.tickets.iter().enumerate() {
                let detail = h.request(Some(&token), &format!("/tickets/{id}")).await;
                if wanted.contains(&index) {
                    assert_eq!(detail.status, 200, "{scope}: {}", detail.text());
                    assert_eq!(detail.json()["id"], id.to_string());
                } else {
                    detail.error(404, "RESOURCE_NOT_FOUND");
                }
            }
            let export = csv(&h.request(Some(&token), "/tickets/export.csv").await);
            assert_eq!(export.len() - 1, wanted.len());
            let numbers: BTreeSet<_> = export[1..].iter().map(|row| row[0].clone()).collect();
            assert_eq!(
                numbers,
                wanted.iter().map(|i| format!("WO-{i:06}")).collect()
            );
        }
        let mut omitted = h
            .scenario(&f, "old-context", "TENANT", vec![], Value::Null)
            .await;
        omitted
            .context
            .as_object_mut()
            .unwrap()
            .remove("organizationScopeIds");
        h.put("old-context", omitted);
        assert_eq!(
            ids(&h.request(Some("old-context"), "/tickets").await.json()),
            expected(&f, &[0, 1])
        );
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn tenant_and_hierarchy_invisibility() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        h.scenario(
            &f,
            "org",
            "ORG_SUBTREE",
            vec![f.org, f.child],
            jids(&[f.org, f.child]),
        )
        .await;
        for index in [4, 5, 6, 7] {
            h.request(Some("org"), &format!("/tickets/{}", f.tickets[index]))
                .await
                .error(404, "RESOURCE_NOT_FOUND");
        }
        h.request(Some("org"), &format!("/tickets/{}", Uuid::new_v4()))
            .await
            .error(404, "RESOURCE_NOT_FOUND");
        let visible = h
            .request(Some("org"), &format!("/tickets/{}", f.tickets[2]))
            .await;
        assert_eq!(visible.status, 200);
        assert_eq!(visible.json()["organizationId"], f.child.to_string());
        assert_eq!(visible.json()["createdAt"], "2026-01-02T03:04:05.123Z");
        for outside in [f.parent, f.peer] {
            let query = format!("organizationId={outside}");
            let filtered = h.request(Some("org"), &format!("/tickets?{query}")).await;
            assert_eq!(filtered.status, 200);
            assert_eq!(filtered.json()["total"], 0);
            assert!(ids(&filtered.json()).is_empty());
            assert_eq!(
                csv(&h
                    .request(Some("org"), &format!("/tickets/export.csv?{query}"))
                    .await)
                .len(),
                1
            );
        }
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn grant_and_permission_denials() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let base = h.scenario(&f, "base", "TENANT", vec![], Value::Null).await;
        let mut cases = Vec::new();
        let mut s = base.clone();
        s.context["permissions"] = json!([]);
        cases.push((s, "OPERATION_DENIED"));
        let mut s = base.clone();
        s.context["dataGrants"] = json!([]);
        cases.push((s, "DATA_DENIED"));
        let mut s = base.clone();
        s.context["dataGrants"][0]["tenantId"] = json!(f.other_tenant.to_string());
        cases.push((s, "DATA_DENIED"));
        let mut s = base.clone();
        s.context["dataGrants"][0]["subjectUserId"] = json!(f.other_user.to_string());
        cases.push((s, "DATA_DENIED"));
        let mut s = base.clone();
        s.context["dataGrants"][0]["scope"] = json!("ALL");
        cases.push((s, "DATA_DENIED"));
        let mut s = base.clone();
        s.context["dataGrants"][0]["organizationIds"] = jids(&[f.org]);
        cases.push((s, "DATA_DENIED"));
        let mut s = base.clone();
        s.context["dataGrants"]
            .as_array_mut()
            .unwrap()
            .push(base.context["dataGrants"][0].clone());
        cases.push((s, "DATA_DENIED"));
        let mut s = base.clone();
        s.context["dataGrants"][0]["scope"] = json!("ORG");
        s.context["dataGrants"][0]["organizationIds"] = jids(&[f.peer]);
        cases.push((s, "DATA_DENIED"));
        let mut s = base.clone();
        s.context["dataGrants"][0]["scope"] = json!("ORG_SUBTREE");
        s.context["dataGrants"][0]["organizationIds"] = jids(&[f.child]);
        cases.push((s, "DATA_DENIED"));
        for (i, (scenario, code)) in cases.into_iter().enumerate() {
            let token = format!("deny-{i}");
            h.put(&token, scenario);
            h.request(Some(&token), "/tickets").await.error(403, code);
        }
        let mut no_export_grant = base.clone();
        no_export_grant.context["dataGrants"]
            .as_array_mut()
            .unwrap()
            .retain(|grant| grant["action"] == "read");
        h.put("no-export-grant", no_export_grant);
        h.request(Some("no-export-grant"), "/tickets/export.csv")
            .await
            .error(403, "DATA_DENIED");
        let mut s = base;
        s.context["permissions"] = json!(["ticket:read"]);
        h.put("no-export", s);
        h.request(Some("no-export"), "/tickets/export.csv")
            .await
            .error(403, "OPERATION_DENIED");
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn invalid_token_and_context_contracts() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        h.request(None, "/tickets")
            .await
            .error(401, "INVALID_APP_TOKEN");
        let base = h.scenario(&f, "base", "TENANT", vec![], Value::Null).await;
        let mut cases = Vec::new();
        for (field, value) in [
            ("active", json!(false)),
            ("aud", json!("another-app")),
            ("aud", json!(["work-orders"])),
            ("exp", json!(1)),
            ("exp", json!("9999999999")),
            ("token_type", json!("bearer")),
            ("sub", json!("not-a-uuid")),
        ] {
            let mut s = base.clone();
            s.intro[field] = value;
            cases.push((s, "INVALID_APP_TOKEN"));
        }
        for (field, value) in [
            ("contractVersion", json!("2.0")),
            ("appCode", json!("another-app")),
            ("contextVersion", json!("invalid")),
            ("expiresAt", json!("2000-01-01T00:00:00Z")),
            ("expiresAt", json!("2099-01-01T00:00:00Z")),
            ("organizationScopeIds", jids(&[f.peer])),
        ] {
            let mut s = base.clone();
            s.context[field] = value;
            cases.push((s, "INVALID_CONTEXT"));
        }
        for path in ["user", "tenant", "organization"] {
            let mut s = base.clone();
            s.context[path]["id"] = json!(Uuid::new_v4().to_string());
            cases.push((s, "INVALID_CONTEXT"));
        }
        for (i, (scenario, code)) in cases.into_iter().enumerate() {
            let token = format!("invalid-{i}");
            h.put(&token, scenario);
            h.request(Some(&token), "/tickets").await.error(401, code);
        }
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn upstream_failures_are_bounded_and_secret_safe() {
    owned(|f| async move {
        let mut h = Harness::new(&f).await;
        let base = h.scenario(&f, "base", "TENANT", vec![], Value::Null).await;
        let mut cases = Vec::new();
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let mut s = base.clone();
            s.intro_status = status;
            cases.push((s, status.as_u16()));
            let mut s = base.clone();
            s.context_status = status;
            cases.push((s, status.as_u16()));
        }
        for mode in [
            Mode::Delay,
            Mode::Oversize,
            Mode::Malformed,
            Mode::Html,
            Mode::Redirect,
        ] {
            let mut s = base.clone();
            s.intro_mode = mode.clone();
            cases.push((s, 503));
            let mut s = base.clone();
            s.context_mode = mode;
            cases.push((s, 503));
        }
        for (i, (scenario, status)) in cases.into_iter().enumerate() {
            let token = format!("upstream-{i}");
            h.put(&token, scenario);
            let started = Instant::now();
            let reply = h.request(Some(&token), "/tickets").await;
            let code = match status {
                401 => "INVALID_APP_TOKEN",
                403 => "DATA_DENIED",
                _ => "PLATFORM_UNAVAILABLE",
            };
            reply.error(if status == 500 { 503 } else { status }, code);
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "bounded upstream authorization"
            );
            assert!(!reply.text().contains("platform detail"));
            assert!(!reply.text().contains("127.0.0.1"));
        }
        assert_eq!(
            h.state.lock().unwrap().redirect_hits,
            0,
            "authorization never follows redirects"
        );
        let platform_task = h.tasks.remove(0);
        platform_task.abort();
        let _ = platform_task.await;
        h.request(Some("base"), "/tickets")
            .await
            .error(503, "PLATFORM_UNAVAILABLE");
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn authorization_is_fresh_on_every_request() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let mut scenario = h
            .scenario(&f, "revocable", "TENANT", vec![], Value::Null)
            .await;
        assert_eq!(h.request(Some("revocable"), "/tickets").await.status, 200);
        scenario.intro["active"] = json!(false);
        h.put("revocable", scenario.clone());
        h.request(Some("revocable"), "/tickets")
            .await
            .error(401, "INVALID_APP_TOKEN");
        scenario.intro["active"] = json!(true);
        scenario.context["permissions"] = json!([]);
        h.put("revocable", scenario);
        h.request(Some("revocable"), "/tickets")
            .await
            .error(403, "OPERATION_DENIED");
        let state = h.state.lock().unwrap();
        assert_eq!(state.calls.iter().filter(|c| c.stage == "intro").count(), 3);
        assert_eq!(
            state.calls.iter().filter(|c| c.stage == "context").count(),
            2
        );
        drop(state);
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn filters_literal_count_and_export_agree() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        h.scenario(&f, "filters", "TENANT", vec![], Value::Null)
            .await;
        let query = query_params(&[
            ("q", "100%_\\"),
            ("status", "IN_PROGRESS"),
            ("priority", "HIGH"),
            ("organizationId", &f.org.to_string()),
        ]);
        let list = h
            .request(Some("filters"), &format!("/tickets?{query}"))
            .await;
        assert_eq!(list.status, 200, "{}", list.text());
        assert_eq!(list.json()["total"], 1);
        assert_eq!(ids(&list.json()), expected(&f, &[0]));
        let export = csv(&h
            .request(Some("filters"), &format!("/tickets/export.csv?{query}"))
            .await);
        assert_eq!(export.len(), 2);
        assert_eq!(export[1][0], "WO-000000");
        for q in ["%", "_", "\\", "' OR TRUE --"] {
            let encoded = query_params(&[("q", q)]);
            let reply = h
                .request(Some("filters"), &format!("/tickets?{encoded}"))
                .await;
            assert_eq!(reply.status, 200);
            assert_eq!(
                reply.json()["total"],
                if q == "' OR TRUE --" { 0 } else { 1 }
            );
        }
        for query in [
            "status=INVALID",
            "priority=invalid",
            "organizationId=invalid",
            "page=1&page=2",
            "q=a&q=b",
            "unknown=1",
            "q=%",
            "q=%FF",
            "q=%00",
        ] {
            let reply = h
                .request(Some("filters"), &format!("/tickets?{query}"))
                .await;
            reply.error(400, "VALIDATION_ERROR");
        }
        let encoded = query_params(&[("q", &"😀".repeat(201))]);
        h.request(Some("filters"), &format!("/tickets?{encoded}"))
            .await
            .error(400, "VALIDATION_ERROR");
        for query in ["page=1", "size=20", "unknown=1", "q=a&q=b"] {
            h.request(Some("filters"), &format!("/tickets/export.csv?{query}"))
                .await
                .error(400, "VALIDATION_ERROR");
        }
        for query in [
            "q=x".to_owned(),
            format!("tenantId={}", f.other_tenant),
            "q=x&q=y".to_owned(),
        ] {
            h.request(
                Some("filters"),
                &format!("/tickets/{}?{query}", f.tickets[0]),
            )
            .await
            .error(400, "VALIDATION_ERROR");
        }
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn pagination_boundaries_and_stable_order() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        h.scenario(&f, "pages", "TENANT", vec![], Value::Null).await;
        for query in [
            "page=1000001",
            "page=18446744073709551616",
            "page=99999999999999999999999999999999",
            "page=0",
            "page=-1",
            "page=1e6",
            "page=1.0",
            "size=0",
            "size=101",
            "size=18446744073709551616",
        ] {
            h.request(Some("pages"), &format!("/tickets?{query}"))
                .await
                .error(400, "VALIDATION_ERROR");
        }
        let last = h
            .request(Some("pages"), "/tickets?page=1000000&size=100")
            .await;
        assert_eq!(last.status, 200);
        assert_eq!(last.json()["total"], 6);
        assert_eq!(last.json()["records"], json!([]));
        let mut collected = Vec::new();
        for page in 1..=3 {
            let reply = h
                .request(Some("pages"), &format!("/tickets?page={page}&size=2"))
                .await;
            assert_eq!(reply.status, 200);
            assert_eq!(reply.json()["page"], page);
            assert_eq!(reply.json()["size"], 2);
            assert_eq!(reply.json()["total"], 6);
            collected.extend(
                reply.json()["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row["id"].as_str().unwrap().to_owned()),
            );
        }
        let mut wanted: Vec<_> = expected(&f, &[0, 1, 2, 3, 4, 5]).into_iter().collect();
        wanted.sort();
        assert_eq!(collected, wanted);
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn csv_encoding_formula_and_scope() {
    owned(|f| async move {
        let h=Harness::new(&f).await;h.scenario(&f,"csv","ORG",vec![f.org],jids(&[f.org])).await;
        sqlx::query("UPDATE work_orders SET title=$1,description=$2,assignee_user_id=$3,assignee_display_name=$4 WHERE id=$5")
            .bind("  =SUM(1,2) 中文\"title\"").bind("\t@danger\r\ncomma,quoted\"body\"")
            .bind(f.other_user).bind("+assignee").bind(f.tickets[0]).execute(&f.pool).await.unwrap();
        let reply=h.request(Some("csv"),"/tickets/export.csv").await;let rows=csv(&reply);
        assert_eq!(rows[0],vec!["编号","标题","描述","优先级","状态","组织","创建人","处理人","创建时间","更新时间"]);
        assert_eq!(rows.len(),3);
        let row=rows.iter().find(|row|row[0]=="WO-000000").unwrap();
        assert_eq!(row[1],"'  =SUM(1,2) 中文\"title\"");assert_eq!(row[2],"'\t@danger\r\ncomma,quoted\"body\"");
        assert_eq!(row[3],"高");assert_eq!(row[4],"处理中");assert_eq!(row[7],"'+assignee");
        assert_eq!(row[8],"2026-01-02T03:04:05.123Z");
        assert!(reply.headers.get("content-disposition").unwrap().to_str().unwrap().contains("attachment"));
        let list=h.request(Some("csv"),"/tickets").await;
        let numbers:BTreeSet<_>=list.json()["records"].as_array().unwrap().iter().map(|r|r["number"].as_str().unwrap().to_owned()).collect();
        assert_eq!(rows[1..].iter().map(|r|r[0].clone()).collect::<BTreeSet<_>>(),numbers);
        h.assert_wire();
    }).await;
}

async fn bulk(f: &Fixture, count: i32, description: &str) {
    sqlx::query("INSERT INTO work_orders(id,number,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,title,description) SELECT md5('phase17-bulk-'||g::text)::uuid,'BULK-'||g::text,$1,$2,'组织',$3,'创建人','bulk title',$4 FROM generate_series(1,$5) g")
        .bind(f.tenant).bind(f.org).bind(f.user).bind(description).bind(count).execute(&f.pool).await.expect("owned bulk seed");
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn csv_row_capacity() {
    owned(|f| async move {
        f.pool.execute("DELETE FROM work_orders").await.unwrap();bulk(&f,1000,"").await;
        let h=Harness::new(&f).await;h.scenario(&f,"capacity","TENANT",vec![],Value::Null).await;
        assert_eq!(csv(&h.request(Some("capacity"),"/tickets/export.csv").await).len(),1001);
        sqlx::query("INSERT INTO work_orders(id,number,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,title) VALUES($1,'OVERFLOW',$2,$3,'组织',$4,'创建人','overflow')")
            .bind(Uuid::new_v4()).bind(f.tenant).bind(f.org).bind(f.user).execute(&f.pool).await.unwrap();
        h.request(Some("capacity"),"/tickets/export.csv").await.error(400,"EXPORT_LIMIT");
        let bounded=h.request(Some("capacity"),"/tickets/export.csv?q=OVERFLOW").await;assert_eq!(csv(&bounded).len(),2);
        h.assert_wire();
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn csv_byte_capacity() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        h.scenario(&f, "bytes", "TENANT", vec![], Value::Null).await;
        f.pool.execute("DELETE FROM work_orders").await.unwrap();
        bulk(&f, 600, &"x".repeat(8192)).await;
        h.request(Some("bytes"), "/tickets/export.csv")
            .await
            .error(413, "EXPORT_LIMIT");
        f.pool.execute("DELETE FROM work_orders").await.unwrap();
        bulk(&f, 300, &"\"".repeat(8192)).await;
        h.request(Some("bytes"), "/tickets/export.csv")
            .await
            .error(413, "EXPORT_LIMIT");
        f.pool.execute("DELETE FROM work_orders").await.unwrap();
        bulk(&f, 1, &"界".repeat(8192)).await;
        assert_eq!(
            csv(&h.request(Some("bytes"), "/tickets/export.csv").await).len(),
            2
        );
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn cors_preflight_and_error_responses() {
    owned(|f| async move {
        let h = Harness::new(&f).await;
        let valid = h
            .client
            .request(
                reqwest::Method::OPTIONS,
                format!("{}{PREFIX}/tickets", h.origin),
            )
            .header("origin", "null")
            .header("access-control-request-method", "GET")
            .header(
                "access-control-request-headers",
                "Authorization, X-Request-Id",
            )
            .send()
            .await
            .unwrap();
        assert_eq!(valid.status(), 204);
        assert_eq!(
            valid.headers().get("access-control-allow-origin").unwrap(),
            "null"
        );
        assert!(!valid
            .headers()
            .contains_key("access-control-allow-credentials"));
        assert!(
            h.state.lock().unwrap().calls.is_empty(),
            "preflight never introspects"
        );
        for (origin, method, headers) in [
            ("https://untrusted.example", "GET", "authorization"),
            ("null", "TRACE", "authorization"),
            ("null", "GET", "cookie"),
        ] {
            let denied = h
                .client
                .request(
                    reqwest::Method::OPTIONS,
                    format!("{}{PREFIX}/tickets", h.origin),
                )
                .header("origin", origin)
                .header("access-control-request-method", method)
                .header("access-control-request-headers", headers)
                .send()
                .await
                .unwrap();
            assert_eq!(denied.status(), 403);
            if origin != "null" {
                assert!(!denied.headers().contains_key("access-control-allow-origin"));
            }
        }
        h.request(None, "/tickets")
            .await
            .error(401, "INVALID_APP_TOKEN");
        h.scenario(&f, "ok", "TENANT", vec![], Value::Null).await;
        h.request(Some("ok"), "/tickets?page=0")
            .await
            .error(400, "VALIDATION_ERROR");
        let supplied = Uuid::new_v4().to_string();
        let response = h
            .client
            .get(format!("{}{PREFIX}/tickets", h.origin))
            .header("origin", "null")
            .header("x-request-id", &supplied)
            .bearer_auth("ok")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get("x-request-id").unwrap(),
            supplied.as_str()
        );
        h.assert_wire();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn representative_query_plan_baseline() {
    owned(|f| async move {
        let orgs:Vec<_>=std::iter::once(f.org).chain((0..11).map(|_|Uuid::new_v4())).collect();
        sqlx::query("INSERT INTO work_orders(id,number,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,title,description,created_at) SELECT md5('phase17-plan-'||g::text)::uuid,'PLAN-'||g::text,CASE WHEN g%5=0 THEN $2 ELSE $1 END,($5::uuid[])[(g%12)+1], '代表组织',CASE WHEN g%2=0 THEN $3 ELSE $4 END,'代表创建人',CASE WHEN g%101=0 THEN 'literal 100%_\\ match' ELSE 'ordinary representative ticket' END,'representative description',now()-g*interval '1 second' FROM generate_series(1,24000) g")
            .bind(f.tenant).bind(f.other_tenant).bind(f.user).bind(f.other_user).bind(&orgs)
            .execute(&f.pool).await.expect("representative dataset");
        f.pool.execute("ANALYZE work_orders").await.unwrap();
        let total:i64=sqlx::query_scalar("SELECT count(*) FROM work_orders").fetch_one(&f.pool).await.unwrap();assert_eq!(total,24008);
        for (name,kind,ceiling,self_only) in [
            ("org_first_page","page",true,false),
            ("org_maximum_offset","deep",true,false),
            ("org_count","count",true,false),
            ("org_literal_search","search",true,false),
            ("org_export_capacity","capacity",true,false),
            ("tenant_first_page","page",false,false),
            ("tenant_count","count",false,false),
            ("self_first_page","page",false,true),
        ] { explain_plan(&f,name,kind,ceiling,self_only,total).await; }
        let h=Harness::new(&f).await;h.scenario(&f,"plan","TENANT",vec![],jids(&[f.org])).await;
        let reply=h.request(Some("plan"),"/tickets?page=1000000&size=100").await;
        assert_eq!(reply.status,200);assert!(reply.json()["records"].as_array().unwrap().is_empty());
        assert_eq!(reply.json()["total"],1602);h.assert_wire();
    }).await;
}

async fn explain_plan(
    f: &Fixture,
    name: &str,
    kind: &str,
    ceiling: bool,
    self_only: bool,
    total: i64,
) {
    // Keep the projection and parameter types aligned with src/reads.rs. This
    // includes timestamp rendering, so the measured page cost covers the real
    // read projection rather than an artificially cheaper SELECT id.
    const RECORDS: &str = "SELECT id,number,title,description,priority,status,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,assignee_user_id,assignee_display_name,version,pg_catalog.to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_iso,pg_catalog.to_char(updated_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_iso FROM public.work_orders WHERE ";
    let prefix = match kind {
        "count" => "SELECT count(*)::bigint FROM public.work_orders WHERE ",
        "capacity" => "WITH selected AS (SELECT number,title,description,organization_name,owner_display_name,assignee_display_name FROM public.work_orders WHERE ",
        _ => RECORDS,
    };
    let mut query = QueryBuilder::<Postgres>::new("EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) ");
    query
        .push(prefix)
        .push("tenant_id=")
        .push_bind(f.tenant)
        .push(" AND deleted_at IS NULL");
    if ceiling {
        query
            .push(" AND organization_id=ANY(")
            .push_bind(vec![f.org])
            .push("::uuid[])");
    }
    if self_only {
        query.push(" AND owner_user_id=").push_bind(f.user);
    }
    if kind == "search" {
        let pattern = "%100\\%\\_\\\\%";
        query
            .push(" AND (title ILIKE ")
            .push_bind(pattern)
            .push(" ESCAPE CHR(92) OR description ILIKE ")
            .push_bind(pattern)
            .push(" ESCAPE CHR(92) OR number ILIKE ")
            .push_bind(pattern)
            .push(" ESCAPE CHR(92))");
    }
    if kind == "capacity" {
        query.push(" ORDER BY created_at DESC,id ASC LIMIT 1001) SELECT count(*)::bigint AS total,COALESCE(sum(octet_length(number)::bigint+octet_length(title)+octet_length(description)+octet_length(organization_name)+octet_length(owner_display_name)+octet_length(COALESCE(assignee_display_name,''))),0)::bigint AS minimum_bytes FROM selected");
    } else if kind != "count" {
        query
            .push(" ORDER BY created_at DESC,id ASC LIMIT ")
            .push_bind(if kind == "deep" || kind == "search" {
                100_i64
            } else {
                20_i64
            })
            .push(" OFFSET ")
            .push_bind(if kind == "deep" {
                99_999_900_i64
            } else {
                0_i64
            });
    }
    let sql = query.sql().to_owned();
    let plans: Vec<String> = query
        .build_query_scalar()
        .fetch_all(&f.pool)
        .await
        .expect("representative plan");
    assert!(plans.iter().any(|line| line.contains("Execution Time:")));
    assert!(plans.iter().any(|line| line.contains("Buffers:")));
    println!(
        "WORK_ORDERS_QUERY_PLAN {}",
        json!({"case":name,"rows":total,"tenantRows":19207,"ceilingRows":1602,"sql":sql,"plan":plans,"newIndexes":[],"parameters":{"tenantId":f.tenant.to_string(),"organizationIds":if ceiling {jids(&[f.org])} else {Value::Null},"ownerUserId":if self_only {json!(f.user.to_string())} else {Value::Null},"pattern":if kind=="search" {json!("%100\\%\\_\\\\%")} else {Value::Null},"maximumOffset":if kind=="deep" {99_999_900} else {0}}})
    );
}
