//! Real, self-owned PostgreSQL migration and adoption regressions.
//! Run explicitly through the owned CI PostgreSQL driver; ordinary unit tests ignore these.
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, Connection, Executor, PgPool};
use std::{future::Future, time::Duration};
use uuid::Uuid;
use work_orders_api::initialize_database;

const SQL: &str = include_str!("../migrations/0001_work_orders.sql");
const IDEMPOTENCY_SQL: &str = include_str!("../migrations/0002_create_idempotency.sql");
const REMINDER_SQL: &str = include_str!("../migrations/0003_reminder_intents.sql");
const LEGACY: &str = "CREATE TABLE work_order_migrations(version TEXT PRIMARY KEY,checksum CHAR(64) NOT NULL,applied_at TIMESTAMPTZ NOT NULL DEFAULT now())";

#[derive(Clone)]
struct Fixture {
    pool: PgPool,
    database_url: String,
    tenant: Uuid,
    ticket: Uuid,
}

async fn cleanup_owned_database(admin: &PgPool, name: &str) -> bool {
    // Only names whose CREATE DATABASE succeeded in this fixture reach cleanup.
    sqlx::query(&format!("DROP DATABASE \"{name}\" WITH (FORCE)"))
        .execute(admin)
        .await
        .is_ok()
}

async fn connect_owned_pool(
    admin: &PgPool,
    name: &str,
    url: &str,
    options: PgPoolOptions,
) -> Result<PgPool, &'static str> {
    match options.connect(url).await {
        Ok(pool) => Ok(pool),
        Err(_) => {
            let removed = cleanup_owned_database(admin, name).await;
            admin.close().await;
            println!(
                "WORK_ORDERS_FIXTURE_SETUP_CLEANUP database_removed={removed} pool_closed=true"
            );
            assert!(removed, "owned setup database cleanup failed");
            Err("owned business pool setup failed")
        }
    }
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
    let name = format!("work_orders_phase16_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&admin)
        .await
        .expect("create owned database");
    url.set_path(&format!("/{name}"));
    let options = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .after_connect(|connection, _| {
            Box::pin(async move {
                connection
                    .execute("SET statement_timeout='10s'; SET lock_timeout='5s'")
                    .await?;
                Ok(())
            })
        });
    let pool = connect_owned_pool(&admin, &name, url.as_str(), options)
        .await
        .expect("owned business pool");
    let result = tokio::spawn(case(Fixture {
        pool: pool.clone(),
        database_url: url.to_string(),
        tenant: Uuid::new_v4(),
        ticket: Uuid::new_v4(),
    }))
    .await;
    pool.close().await;
    let removed = cleanup_owned_database(&admin, &name).await;
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

async fn legacy(f: &Fixture) {
    f.pool.execute(SQL).await.expect("exact historical DDL");
    f.pool.execute(LEGACY).await.expect("historical ledger");
    sqlx::query(
        "INSERT INTO work_order_migrations(version,checksum) VALUES('0001_work_orders.sql',$1)",
    )
    .bind(format!("{:x}", Sha256::digest(SQL.as_bytes())))
    .execute(&f.pool)
    .await
    .expect("historical checksum");
}

async fn sqlx_v1(f: &Fixture) {
    use sqlx::migrate::{Migrate, Migration, MigrationType};
    legacy(f).await;
    let migration = Migration::new(
        1,
        "work orders".into(),
        MigrationType::Simple,
        SQL.into(),
        false,
    );
    let mut connection = f.pool.acquire().await.expect("old SQLx connection");
    connection
        .ensure_migrations_table()
        .await
        .expect("old SQLx ledger");
    sqlx::query("INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(1,'work orders',true,$1,0)")
        .bind(migration.checksum.as_ref()).execute(&mut *connection).await.expect("old SQLx checksum");
}

async fn append_v2(f: &Fixture, sqlx: bool) {
    use sqlx::migrate::{Migration, MigrationType};
    f.pool
        .execute(IDEMPOTENCY_SQL)
        .await
        .expect("historical v2 DDL");
    sqlx::query("INSERT INTO work_order_migrations(version,checksum) VALUES('0002_create_idempotency.sql',$1)")
        .bind(format!("{:x}", Sha256::digest(IDEMPOTENCY_SQL.as_bytes())))
        .execute(&f.pool).await.expect("historical v2 compatibility checksum");
    if sqlx {
        let migration = Migration::new(
            2,
            "create idempotency".into(),
            MigrationType::Simple,
            IDEMPOTENCY_SQL.into(),
            false,
        );
        sqlx::query("INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(2,'create idempotency',true,$1,0)")
            .bind(migration.checksum.as_ref()).execute(&f.pool).await.expect("historical v2 SQLx checksum");
    }
}

async fn sqlx_v2(f: &Fixture) {
    sqlx_v1(f).await;
    append_v2(f, true).await;
}

async fn reminder_intent(f: &Fixture) -> String {
    let (organization, user): (Uuid, Uuid) =
        sqlx::query_as("SELECT organization_id,owner_user_id FROM work_orders WHERE id=$1")
            .bind(f.ticket)
            .fetch_one(&f.pool)
            .await
            .expect("intent ticket identity");
    let payload = serde_json::json!({
        "idempotencyKey":"preserved-reminder", "target":{"kind":"USER","organizationId":organization.to_string(),"userId":user.to_string()},
        "type":"INFO", "title":"工单提醒 WO-000041", "content":"请查看工单 WO-000041 的详情并处理。",
        "link":format!("/micro/work-orders/tickets/{}",f.ticket),
    }).to_string();
    sqlx::query("INSERT INTO work_order_reminder_intents(tenant_id,organization_id,application_id,user_id,idempotency_key,ticket_id,target_organization_id,recipient_user_id,request_hash,request_json) VALUES($1,$2,$3,$4,'preserved-reminder',$5,$2,$4,$6,$7)")
        .bind(f.tenant).bind(organization).bind(Uuid::new_v4()).bind(user).bind(f.ticket)
        .bind(format!("{:x}",Sha256::digest(payload.as_bytes()))).bind(payload)
        .execute(&f.pool).await.expect("existing immutable reminder intent");
    sqlx::query_scalar("SELECT row_to_json(t)::text FROM work_order_reminder_intents t")
        .fetch_one(&f.pool)
        .await
        .expect("intent snapshot")
}

async fn journal(f: &Fixture, sqlx: bool) -> Vec<String> {
    let statement = if sqlx {
        "SELECT row_to_json(t)::text FROM _sqlx_migrations t ORDER BY version"
    } else {
        "SELECT row_to_json(t)::text FROM work_order_migrations t ORDER BY version"
    };
    sqlx::query_scalar(statement)
        .fetch_all(&f.pool)
        .await
        .expect("historical journal")
}

async fn node_migrate(f: &Fixture) {
    let url = f.database_url.clone();
    let output = tokio::task::spawn_blocking(move || std::process::Command::new("node")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["--input-type=module", "-e", "import {createPool,migrate} from './db.mjs'; const p=createPool({databaseUrl:process.env.WORK_ORDERS_DATABASE_URL});try{await migrate(p);console.log('LEGACY_NODE_MIGRATED')}catch{process.exitCode=1}finally{await p.end()}"])
        .env("WORK_ORDERS_DATABASE_URL",url).output().expect("legacy Node migration driver"))
        .await.expect("legacy migration join");
    assert!(
        output.status.success(),
        "unchanged Node migration runner must accept the pinned migration sequence"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("LEGACY_NODE_MIGRATED"));
}

async fn receipt(f: &Fixture) -> String {
    sqlx::query("INSERT INTO work_order_create_idempotency(tenant_id,organization_id,user_id,action,idempotency_key,request_hash,ticket_id,response_json) VALUES($1,$2,$3,'create','preserved-key',repeat('a',64),$4,'{\"id\":\"historical-result\"}')")
        .bind(f.tenant).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(f.ticket)
        .execute(&f.pool).await.expect("existing completed receipt");
    sqlx::query_scalar("SELECT row_to_json(t)::text FROM work_order_create_idempotency t")
        .fetch_one(&f.pool)
        .await
        .expect("existing receipt snapshot")
}

async fn assert_current(f: &Fixture) {
    assert_eq!(journal(f, false).await.len(), 3);
    assert_eq!(journal(f, true).await.len(), 3);
    let checksum: String = sqlx::query_scalar(
        "SELECT checksum FROM work_order_migrations WHERE version='0001_work_orders.sql'",
    )
    .fetch_one(&f.pool)
    .await
    .expect("immutable legacy baseline");
    assert_eq!(
        checksum,
        "a2e4224febd195fdd663a8954c3be4b76a8a06338521056c6809ebaa57d04090"
    );
    let checksum: String = sqlx::query_scalar(
        "SELECT checksum FROM work_order_migrations WHERE version='0002_create_idempotency.sql'",
    )
    .fetch_one(&f.pool)
    .await
    .expect("incremental legacy checksum");
    assert_eq!(
        checksum,
        format!("{:x}", Sha256::digest(IDEMPOTENCY_SQL.as_bytes()))
    );
    let checksum: String = sqlx::query_scalar(
        "SELECT checksum FROM work_order_migrations WHERE version='0003_reminder_intents.sql'",
    )
    .fetch_one(&f.pool)
    .await
    .expect("reminder checksum");
    assert_eq!(
        checksum,
        format!("{:x}", Sha256::digest(REMINDER_SQL.as_bytes()))
    );
    let references: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_namespace WHERE nspname LIKE 'work_order_baseline_%'",
    )
    .fetch_one(&f.pool)
    .await
    .expect("temporary reference cleanup");
    assert_eq!(references, 0);
}

async fn seed(f: &Fixture) {
    sqlx::query("INSERT INTO work_order_counters(tenant_id,next_number) VALUES($1,42)")
        .bind(f.tenant)
        .execute(&f.pool)
        .await
        .expect("legacy counter");
    sqlx::query("INSERT INTO work_orders(id,number,tenant_id,organization_id,organization_name,owner_user_id,owner_display_name,title,description,version,created_at,updated_at) VALUES($1,'WO-000041',$2,$3,'Legacy organization',$4,'Legacy owner','Historical ticket','preserve me',7,'2025-01-02T03:04:05Z','2025-02-03T04:05:06Z')")
        .bind(f.ticket).bind(f.tenant).bind(Uuid::new_v4()).bind(Uuid::new_v4()).execute(&f.pool).await.expect("legacy ticket");
}

async fn data(f: &Fixture) -> (String, i64) {
    let ticket = sqlx::query_scalar::<_, String>(
        "SELECT row_to_json(t)::text FROM work_orders t WHERE id=$1",
    )
    .bind(f.ticket)
    .fetch_one(&f.pool)
    .await
    .expect("historical row");
    let counter = sqlx::query_scalar::<_, i64>(
        "SELECT next_number FROM work_order_counters WHERE tenant_id=$1",
    )
    .bind(f.tenant)
    .fetch_one(&f.pool)
    .await
    .expect("counter");
    (ticket, counter)
}

async fn no_sqlx(f: &Fixture) {
    let table = sqlx::query_scalar::<_, Option<String>>(
        "SELECT to_regclass('public._sqlx_migrations')::text",
    )
    .fetch_one(&f.pool)
    .await
    .expect("catalog read");
    assert!(
        table.is_none(),
        "failed adoption must not leave SQLx metadata"
    );
}

async fn drift(sql: &'static str) {
    owned(move |f| async move {
        legacy(&f).await;
        seed(&f).await;
        f.pool.execute(sql).await.expect("controlled drift");
        // The fixture deliberately changes the schema. Freeze that resulting
        // row shape so added/dropped columns are not mistaken for startup writes.
        let original = data(&f).await;
        assert!(
            initialize_database(&f.pool).await.is_err(),
            "schema drift must stop startup"
        );
        no_sqlx(&f).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT next_number FROM work_order_counters WHERE tenant_id=$1"
            )
            .bind(f.tenant)
            .fetch_one(&f.pool)
            .await
            .expect("counter unchanged"),
            42
        );
        // Drift tests never authorize a rewrite of historical business rows or counters.
        assert_eq!(data(&f).await, original);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn empty_first_start_and_repeat() {
    owned(|f| async move {
        assert!(initialize_database(&f.pool).await.is_ok());
        assert!(initialize_database(&f.pool).await.is_ok());
        let entries: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM _sqlx_migrations WHERE success AND octet_length(checksum)=48",
        )
        .fetch_one(&f.pool)
        .await
        .expect("SQLx SHA384 metadata");
        assert_eq!(entries, 3);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_order_migrations")
                .fetch_one(&f.pool)
                .await
                .expect("compat ledger"),
            3
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn legacy_data_and_counter_preserved() {
    owned(|f| async move {
        legacy(&f).await; seed(&f).await;
        let before = data(&f).await;
        assert!(initialize_database(&f.pool).await.is_ok());
        assert!(initialize_database(&f.pool).await.is_ok());
        assert_eq!(data(&f).await, before);
        assert!(sqlx::query("UPDATE work_orders SET organization_name='moved' WHERE id=$1").bind(f.ticket).execute(&f.pool).await.is_err());
        assert_eq!(data(&f).await, before);
        // A legitimate next counter allocation continues at its historical value.
        assert_eq!(sqlx::query_scalar::<_,i64>("UPDATE work_order_counters SET next_number=next_number+1 WHERE tenant_id=$1 RETURNING next_number-1").bind(f.tenant).fetch_one(&f.pool).await.expect("next allocation"),42);
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn concurrent_fresh_start() {
    owned(|f| async move {
        let (a, b) = tokio::join!(initialize_database(&f.pool), initialize_database(&f.pool));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations")
                .fetch_one(&f.pool)
                .await
                .expect("single migration"),
            3
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn concurrent_legacy_takeover() {
    owned(|f| async move {
        legacy(&f).await;
        seed(&f).await;
        let before = data(&f).await;
        let (a, b) = tokio::join!(initialize_database(&f.pool), initialize_database(&f.pool));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(data(&f).await, before);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn node_and_rust_share_migration_lock() {
    owned(|f| async move {
        let mut blocker=f.pool.acquire().await.expect("lock connection");
        let mut tx=blocker.begin().await.expect("lock transaction");
        // Hold the same lock before starting either initializer.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('work-orders-business-migrations',11))")
            .execute(&mut *tx).await.expect("legacy lock");
        let node_url=f.database_url.clone();
        let node=tokio::task::spawn_blocking(move || std::process::Command::new("node")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["--input-type=module","-e","import {createPool,migrate} from './db.mjs'; const p=createPool({databaseUrl:process.env.WORK_ORDERS_DATABASE_URL});try{await migrate(p);console.log('LEGACY_NODE_MIGRATED')}catch{process.exitCode=1}finally{await p.end()}"])
            .env("WORK_ORDERS_DATABASE_URL",node_url).output().expect("legacy Node test driver"));
        let p=f.pool.clone();let rust=tokio::spawn(async move{initialize_database(&p).await});
        // Observe both real connections waiting on the held lock, not an elapsed sleep.
        let deadline=tokio::time::Instant::now()+Duration::from_secs(8);
        loop {
            let waits: i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory'")
                .fetch_one(&f.pool).await.expect("waiter observation");
            if waits>=2{break}assert!(tokio::time::Instant::now()<deadline,"both initializers must contend on legacy lock");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tx.commit().await.expect("release shared lock");drop(blocker);
        let node=node.await.expect("legacy driver join");assert!(node.status.success());assert!(String::from_utf8_lossy(&node.stdout).contains("LEGACY_NODE_MIGRATED"));
        assert!(rust.await.expect("Rust initializer join").is_ok());
        assert!(initialize_database(&f.pool).await.is_ok());
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn legacy_checksum_mismatch_rejected() {
    drift("UPDATE work_order_migrations SET checksum=repeat('0',64)").await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn partial_and_unknown_legacy_history_rejected() {
    drift("DELETE FROM work_order_migrations").await;
    drift("INSERT INTO work_order_migrations(version,checksum) VALUES('9999_unknown.sql',repeat('0',64))").await;
    drift("DROP TABLE work_order_migrations").await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn column_shape_drift_rejected() {
    drift("ALTER TABLE work_orders DROP COLUMN description").await;
    drift("ALTER TABLE work_orders ALTER COLUMN organization_name TYPE varchar(101)").await;
    drift("ALTER TABLE work_orders ADD COLUMN unexpected text").await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn index_drift_rejected() {
    drift("DROP INDEX work_orders_tenant_org_active").await;
    drift("DROP INDEX work_orders_tenant_owner_active;CREATE INDEX work_orders_tenant_owner_active ON work_orders(tenant_id,owner_user_id,created_at DESC,id)").await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn constraint_drift_rejected() {
    drift("ALTER TABLE work_order_counters DROP CONSTRAINT work_order_counters_next_number_check")
        .await;
    drift("ALTER TABLE work_orders DROP CONSTRAINT work_orders_version_check;ALTER TABLE work_orders ADD CONSTRAINT work_orders_version_check CHECK(version>=0)").await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn trigger_function_drift_rejected() {
    drift("CREATE OR REPLACE FUNCTION work_order_identity_immutable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$").await;
    drift("ALTER FUNCTION work_order_identity_immutable() SECURITY DEFINER").await;
    drift("ALTER FUNCTION work_order_identity_immutable() SET search_path=public").await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn disabled_or_missing_trigger_rejected() {
    drift("ALTER TABLE work_orders DISABLE TRIGGER work_order_identity_immutable").await;
    drift("DROP TRIGGER work_order_identity_immutable ON work_orders").await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn sqlx_dirty_checksum_and_unknown_history_rejected() {
    for sql in ["UPDATE _sqlx_migrations SET success=false","UPDATE _sqlx_migrations SET checksum=decode(repeat('00',48),'hex')","INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(9999,'unknown',true,decode(repeat('00',48),'hex'),0)"] {
        owned(move |f| async move {
            assert!(initialize_database(&f.pool).await.is_ok());seed(&f).await;let before=data(&f).await;
            f.pool.execute(sql).await.expect("SQLx drift");
            assert!(initialize_database(&f.pool).await.is_err());assert_eq!(data(&f).await,before);
        }).await;
    }
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn unrelated_database_is_rejected() {
    owned(|f| async move {
        f.pool
            .execute("CREATE TABLE users(id integer PRIMARY KEY);INSERT INTO users VALUES(7)")
            .await
            .expect("foreign schema fixture");
        assert!(initialize_database(&f.pool).await.is_err());
        no_sqlx(&f).await;
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT id FROM users")
                .fetch_one(&f.pool)
                .await
                .expect("foreign row"),
            7
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn migration_failure_rolls_back_and_can_retry() {
    owned(|f| async move {
        // Exercise the actual setup-error cleanup branch with a rejected pool,
        // then confirm the successfully-created auxiliary database is absent.
        let raw = std::env::var("TEST_DATABASE_URL").expect("owned admin URL");
        let admin = PgPoolOptions::new().max_connections(1).connect(&raw).await.expect("owned setup regression admin");
        let name = format!("work_orders_phase16_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE \"{name}\"")).execute(&admin).await.expect("create owned setup regression database");
        let mut url = url::Url::parse(&raw).expect("owned URL");
        url.set_path(&format!("/{name}"));
        let options = PgPoolOptions::new().max_connections(1).acquire_timeout(Duration::from_millis(250))
            .after_connect(|_, _| Box::pin(async { Err(sqlx::Error::Protocol("controlled fixture setup failure".into())) }));
        assert!(connect_owned_pool(&admin, &name, url.as_str(), options).await.is_err());
        assert!(admin.is_closed());
        assert!(!sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_database WHERE datname=$1)")
            .bind(&name).fetch_one(&f.pool).await.expect("owned setup cleanup catalog"));
        f.pool.execute("CREATE SCHEMA phase16_fault;CREATE FUNCTION phase16_fault.fail_index() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN IF TG_TAG='CREATE INDEX' THEN RAISE EXCEPTION 'controlled migration failure';END IF;END $$;CREATE EVENT TRIGGER phase16_fail_index ON ddl_command_start EXECUTE FUNCTION phase16_fault.fail_index()")
            .await.expect("owned failure injector");
        assert!(initialize_database(&f.pool).await.is_err());no_sqlx(&f).await;
        assert!(sqlx::query_scalar::<_,Option<String>>("SELECT to_regclass('public.work_orders')::text").fetch_one(&f.pool).await.expect("rollback catalog").is_none());
        f.pool.execute("DROP EVENT TRIGGER phase16_fail_index;DROP SCHEMA phase16_fault CASCADE").await.expect("remove owned failure injector");
        assert!(initialize_database(&f.pool).await.is_ok());
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn sqlx_v1_upgrade_preserves_history_and_business_rows() {
    owned(|f| async move {
        sqlx_v1(&f).await;
        seed(&f).await;
        let before = data(&f).await;
        let old_legacy = journal(&f, false).await;
        let old_sqlx = journal(&f, true).await;
        assert_eq!(
            initialize_database(&f.pool).await.unwrap(),
            work_orders_api::InitializationReport::Upgraded
        );
        assert_current(&f).await;
        assert_eq!(data(&f).await, before);
        assert_eq!(journal(&f, false).await[0], old_legacy[0]);
        assert_eq!(journal(&f, true).await[0], old_sqlx[0]);
        let current_legacy = journal(&f, false).await;
        let current_sqlx = journal(&f, true).await;
        assert_eq!(
            initialize_database(&f.pool).await.unwrap(),
            work_orders_api::InitializationReport::AlreadyCurrent
        );
        assert_eq!(journal(&f, false).await, current_legacy);
        assert_eq!(journal(&f, true).await, current_sqlx);
        assert_eq!(data(&f).await, before);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn node_v2_and_mixed_sqlx_v1_are_adopted_without_replay() {
    for with_sqlx in [false, true] {
        owned(move |f| async move {
            if with_sqlx {
                sqlx_v1(&f).await;
            } else {
                legacy(&f).await;
            }
            seed(&f).await;
            let before = data(&f).await;
            let old_sqlx = if with_sqlx {
                journal(&f, true).await
            } else {
                Vec::new()
            };
            // Materialize the exact historical Node-v2 prefix. The preserved
            // runner now sees 0003 as well; its real v3 interoperability has a
            // separate test below and the shared-lock test above.
            append_v2(&f, false).await;
            let legacy_before = journal(&f, false).await;
            assert_eq!(legacy_before.len(), 2);
            let receipt_before = receipt(&f).await;
            let expected = if with_sqlx {
                work_orders_api::InitializationReport::Upgraded
            } else {
                work_orders_api::InitializationReport::AdoptedLegacy
            };
            assert_eq!(initialize_database(&f.pool).await.unwrap(), expected);
            assert_current(&f).await;
            assert_eq!(&journal(&f, false).await[..2], legacy_before.as_slice());
            if with_sqlx {
                assert_eq!(journal(&f, true).await[0], old_sqlx[0]);
            }
            assert_eq!(data(&f).await, before);
            assert_eq!(
                sqlx::query_scalar::<_, String>(
                    "SELECT row_to_json(t)::text FROM work_order_create_idempotency t"
                )
                .fetch_one(&f.pool)
                .await
                .unwrap(),
                receipt_before
            );
            // The real Node runner may run after Rust; no known DDL is replayed.
            node_migrate(&f).await;
            assert!(initialize_database(&f.pool).await.is_ok());
            assert_eq!(data(&f).await, before);
        })
        .await;
    }
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn incremental_history_mismatch_is_rejected_without_repair() {
    for sql in [
        "UPDATE work_order_migrations SET checksum=repeat('0',64) WHERE version='0002_create_idempotency.sql'",
        "DELETE FROM work_order_migrations WHERE version='0002_create_idempotency.sql'",
        "UPDATE _sqlx_migrations SET checksum=decode(repeat('00',48),'hex') WHERE version=2",
        "UPDATE _sqlx_migrations SET success=false WHERE version=2",
        "UPDATE _sqlx_migrations SET description='wrong' WHERE version=2",
        "DELETE FROM _sqlx_migrations WHERE version=1",
        "DELETE FROM work_order_migrations WHERE version='0001_work_orders.sql'",
    ] {
        owned(move |f| async move {
            initialize_database(&f.pool).await.unwrap();
            seed(&f).await;
            let business_before = data(&f).await;
            f.pool.execute(sql).await.expect("controlled incremental history drift");
            let legacy_before = journal(&f, false).await;
            let sqlx_before = journal(&f, true).await;
            assert!(initialize_database(&f.pool).await.is_err());
            assert_eq!(data(&f).await, business_before);
            assert_eq!(journal(&f, false).await, legacy_before);
            assert_eq!(journal(&f, true).await, sqlx_before);
        }).await;
    }
    // History claiming 0002 without its table cannot authorize replay or repair.
    owned(|f| async move {
        initialize_database(&f.pool).await.unwrap();
        seed(&f).await;
        let before = data(&f).await;
        f.pool
            .execute("DROP TABLE work_order_create_idempotency")
            .await
            .unwrap();
        assert!(initialize_database(&f.pool).await.is_err());
        assert_eq!(data(&f).await, before);
        assert!(sqlx::query_scalar::<_, Option<String>>(
            "SELECT to_regclass('public.work_order_create_idempotency')::text"
        )
        .fetch_one(&f.pool)
        .await
        .unwrap()
        .is_none());
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn idempotency_catalog_drift_is_rejected() {
    for sql in [
        "ALTER TABLE work_order_create_idempotency DROP CONSTRAINT work_order_create_idempotency_ticket_id_fkey",
        "ALTER TABLE work_order_create_idempotency DROP CONSTRAINT work_order_create_idempotency_idempotency_key_check",
        "ALTER TABLE work_order_create_idempotency DROP CONSTRAINT work_order_create_idempotency_request_hash_check",
        "ALTER TABLE work_order_create_idempotency DROP CONSTRAINT work_order_create_idempotency_pkey",
        "ALTER TABLE work_order_create_idempotency DISABLE TRIGGER ALL",
        "ALTER TABLE work_order_create_idempotency ADD COLUMN unexpected text",
    ] {
        owned(move |f| async move {
            initialize_database(&f.pool).await.unwrap();
            seed(&f).await;
            receipt(&f).await;
            f.pool.execute(sql).await.expect("controlled receipt catalog drift");
            let before = data(&f).await;
            let receipt_before: String = sqlx::query_scalar("SELECT row_to_json(t)::text FROM work_order_create_idempotency t").fetch_one(&f.pool).await.unwrap();
            assert!(initialize_database(&f.pool).await.is_err());
            assert_eq!(data(&f).await, before);
            assert_eq!(sqlx::query_scalar::<_,String>("SELECT row_to_json(t)::text FROM work_order_create_idempotency t").fetch_one(&f.pool).await.unwrap(), receipt_before);
        }).await;
    }
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn incremental_ddl_failure_rolls_back_both_histories_and_retries() {
    owned(|f| async move {
        sqlx_v1(&f).await;
        seed(&f).await;
        let before = data(&f).await;
        let legacy_before = journal(&f, false).await;
        let sqlx_before = journal(&f, true).await;
        // Only the real target CREATE fails. Reference-schema construction is
        // allowed, so this proves rollback after the existing schema was checked.
        f.pool.execute("CREATE SCHEMA phase18_fault;CREATE FUNCTION phase18_fault.fail_receipt() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN IF TG_TAG='CREATE TABLE' AND current_schema()='public' AND current_query() LIKE '%CREATE TABLE work_order_create_idempotency%' THEN RAISE EXCEPTION 'controlled incremental migration failure';END IF;END $$;CREATE EVENT TRIGGER phase18_fail_receipt ON ddl_command_start EXECUTE FUNCTION phase18_fault.fail_receipt()")
            .await.expect("owned incremental fault injector");
        assert!(initialize_database(&f.pool).await.is_err());
        assert_eq!(data(&f).await, before);
        assert_eq!(journal(&f, false).await, legacy_before);
        assert_eq!(journal(&f, true).await, sqlx_before);
        assert!(sqlx::query_scalar::<_,Option<String>>("SELECT to_regclass('public.work_order_create_idempotency')::text").fetch_one(&f.pool).await.unwrap().is_none());
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM pg_namespace WHERE nspname LIKE 'work_order_baseline_%'").fetch_one(&f.pool).await.unwrap(),0);
        f.pool.execute("DROP EVENT TRIGGER phase18_fail_receipt;DROP SCHEMA phase18_fault CASCADE").await.expect("remove owned injector");
        assert!(initialize_database(&f.pool).await.is_ok());
        assert_current(&f).await;
        assert_eq!(data(&f).await, before);
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn concurrent_v1_upgrade_preserves_receipt_constraints() {
    owned(|f| async move {
        sqlx_v1(&f).await;
        seed(&f).await;
        let before = data(&f).await;
        let (a,b)=tokio::join!(initialize_database(&f.pool), initialize_database(&f.pool));
        assert!(a.is_ok() && b.is_ok());
        assert_current(&f).await;
        assert_eq!(data(&f).await,before);
        let organization = Uuid::new_v4();
        let user = Uuid::new_v4();
        let insert = "INSERT INTO work_order_create_idempotency(tenant_id,organization_id,user_id,action,idempotency_key,request_hash,ticket_id,response_json) VALUES($1,$2,$3,$4,$5,$6,$7,$8)";
        let mut transaction = f.pool.begin().await.unwrap();
        sqlx::query(insert).bind(f.tenant).bind(organization).bind(user).bind("create").bind("pending").bind("a".repeat(64)).bind(None::<Uuid>).bind(None::<String>)
            .execute(&mut *transaction).await.expect("receipt placeholder is valid within a transaction");
        transaction.rollback().await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM work_order_create_idempotency").fetch_one(&f.pool).await.unwrap(),0);
        for (action,key,hash,ticket,response) in [
            ("update","key".into(),"a".repeat(64),None,None),
            ("create","".into(),"a".repeat(64),None,None),
            ("create","含中文".into(),"a".repeat(64),None,None),
            ("create","with space".into(),"a".repeat(64),None,None),
            ("create","x".repeat(129),"a".repeat(64),None,None),
            ("create","key".into(),"a".repeat(63),None,None),
            ("create","key".into(),"A".repeat(64),None,None),
            ("create","key".into(),"a".repeat(64),Some(f.ticket),None),
            ("create","key".into(),"a".repeat(64),None,Some("{}")),
            ("create","key".into(),"a".repeat(64),Some(Uuid::new_v4()),Some("{}")),
        ] {
            assert!(sqlx::query(insert).bind(f.tenant).bind(organization).bind(user).bind(action).bind(key).bind(hash).bind(ticket).bind(response).execute(&f.pool).await.is_err());
        }
        for current_user in [user,Uuid::new_v4()] {
            sqlx::query(insert).bind(f.tenant).bind(organization).bind(current_user).bind("create").bind("valid._:-key").bind("a".repeat(64)).bind(f.ticket).bind("{\"version\":1}")
                .execute(&f.pool).await.expect("same key in separate user domain");
        }
        assert!(sqlx::query(insert).bind(f.tenant).bind(organization).bind(user).bind("create").bind("valid._:-key").bind("a".repeat(64)).bind(f.ticket).bind("{\"version\":1}").execute(&f.pool).await.is_err());
        assert!(initialize_database(&f.pool).await.is_ok());
        assert_eq!(data(&f).await,before);
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn sqlx_v2_upgrade_preserves_creation_receipt_and_history() {
    owned(|f| async move {
        sqlx_v2(&f).await;
        seed(&f).await;
        let before = data(&f).await;
        let receipt_before = receipt(&f).await;
        let legacy_before = journal(&f, false).await;
        let sqlx_before = journal(&f, true).await;
        assert_eq!(
            initialize_database(&f.pool).await.unwrap(),
            work_orders_api::InitializationReport::Upgraded
        );
        assert_current(&f).await;
        assert_eq!(&journal(&f, false).await[..2], legacy_before.as_slice());
        assert_eq!(&journal(&f, true).await[..2], sqlx_before.as_slice());
        assert_eq!(data(&f).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT row_to_json(t)::text FROM work_order_create_idempotency t"
            )
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            receipt_before
        );
        let intent_before = reminder_intent(&f).await;
        assert_eq!(
            initialize_database(&f.pool).await.unwrap(),
            work_orders_api::InitializationReport::AlreadyCurrent
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT row_to_json(t)::text FROM work_order_reminder_intents t"
            )
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            intent_before
        );
        assert_eq!(data(&f).await, before);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn node_v3_and_mixed_sqlx_prefixes_preserve_reminder_intents() {
    for sqlx_prefix in 0..=2 {
        owned(move |f| async move {
            match sqlx_prefix {
                0 => legacy(&f).await,
                1 => sqlx_v1(&f).await,
                _ => sqlx_v2(&f).await,
            }
            seed(&f).await;
            let before = data(&f).await;
            let old_sqlx = if sqlx_prefix == 0 {
                Vec::new()
            } else {
                journal(&f, true).await
            };
            node_migrate(&f).await;
            let legacy_before = journal(&f, false).await;
            assert_eq!(legacy_before.len(), 3);
            let receipt_before = receipt(&f).await;
            let intent_before = reminder_intent(&f).await;
            let expected = if sqlx_prefix == 0 {
                work_orders_api::InitializationReport::AdoptedLegacy
            } else {
                work_orders_api::InitializationReport::Upgraded
            };
            assert_eq!(initialize_database(&f.pool).await.unwrap(), expected);
            assert_current(&f).await;
            assert_eq!(journal(&f, false).await, legacy_before);
            assert_eq!(&journal(&f, true).await[..sqlx_prefix], old_sqlx.as_slice());
            assert_eq!(data(&f).await, before);
            assert_eq!(
                sqlx::query_scalar::<_, String>(
                    "SELECT row_to_json(t)::text FROM work_order_create_idempotency t"
                )
                .fetch_one(&f.pool)
                .await
                .unwrap(),
                receipt_before
            );
            assert_eq!(
                sqlx::query_scalar::<_, String>(
                    "SELECT row_to_json(t)::text FROM work_order_reminder_intents t"
                )
                .fetch_one(&f.pool)
                .await
                .unwrap(),
                intent_before
            );
        })
        .await;
    }
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn reminder_intents_are_immutable_and_application_domain_scoped() {
    owned(|f| async move {
        initialize_database(&f.pool).await.unwrap();
        seed(&f).await;
        let before = data(&f).await;
        let intent_before = reminder_intent(&f).await;
        for statement in [
            "UPDATE work_order_reminder_intents SET recipient_user_id=gen_random_uuid()",
            "UPDATE work_order_reminder_intents SET request_json=request_json",
            "DELETE FROM work_order_reminder_intents",
            "INSERT INTO work_order_reminder_intents SELECT tenant_id,organization_id,application_id,user_id,idempotency_key,gen_random_uuid(),target_organization_id,recipient_user_id,request_hash,request_json,created_at FROM work_order_reminder_intents",
        ] {
            assert!(f.pool.execute(statement).await.is_err(), "an existing event cannot be redirected or recycled");
        }
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT row_to_json(t)::text FROM work_order_reminder_intents t").fetch_one(&f.pool).await.unwrap(), intent_before);
        f.pool.execute("INSERT INTO work_order_reminder_intents SELECT tenant_id,organization_id,gen_random_uuid(),user_id,idempotency_key,ticket_id,target_organization_id,recipient_user_id,request_hash,request_json,created_at FROM work_order_reminder_intents")
            .await.expect("a distinct trusted application is a distinct platform domain");
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_order_reminder_intents").fetch_one(&f.pool).await.unwrap(), 2);
        assert!(initialize_database(&f.pool).await.is_ok());
        assert_eq!(data(&f).await, before);
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn reminder_history_and_catalog_drift_is_rejected_without_repair() {
    for statement in [
        "UPDATE work_order_migrations SET checksum=repeat('0',64) WHERE version='0003_reminder_intents.sql'",
        "DELETE FROM work_order_migrations WHERE version='0003_reminder_intents.sql'",
        "UPDATE _sqlx_migrations SET checksum=decode(repeat('00',48),'hex') WHERE version=3",
        "UPDATE _sqlx_migrations SET success=false WHERE version=3",
        "INSERT INTO work_order_migrations(version,checksum) VALUES('0004_unknown.sql',repeat('0',64))",
        "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(4,'unknown',true,decode(repeat('00',48),'hex'),0)",
        "DROP TABLE work_order_reminder_intents",
        "ALTER TABLE work_order_reminder_intents DROP CONSTRAINT work_order_reminder_intents_pkey",
        "ALTER TABLE work_order_reminder_intents ADD COLUMN unexpected text",
        "ALTER TABLE work_order_reminder_intents DISABLE TRIGGER work_order_reminder_intent_immutable",
        "DROP TRIGGER work_order_reminder_intent_immutable ON work_order_reminder_intents",
        "CREATE OR REPLACE FUNCTION work_order_reminder_intent_immutable() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END; $$",
    ] {
        owned(move |f| async move {
            initialize_database(&f.pool).await.unwrap();
            seed(&f).await;
            let before = data(&f).await;
            let receipt_before = receipt(&f).await;
            reminder_intent(&f).await;
            f.pool.execute(statement).await.expect("controlled reminder history or schema drift");
            let legacy_before = journal(&f, false).await;
            let sqlx_before = journal(&f, true).await;
            assert!(initialize_database(&f.pool).await.is_err());
            assert_eq!(data(&f).await, before);
            assert_eq!(journal(&f, false).await, legacy_before);
            assert_eq!(journal(&f, true).await, sqlx_before);
            assert_eq!(sqlx::query_scalar::<_, String>("SELECT row_to_json(t)::text FROM work_order_create_idempotency t").fetch_one(&f.pool).await.unwrap(), receipt_before);
            assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_namespace WHERE nspname LIKE 'work_order_baseline_%'").fetch_one(&f.pool).await.unwrap(), 0);
        }).await;
    }
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn reminder_ddl_failure_rolls_back_v2_and_can_retry() {
    owned(|f| async move {
        sqlx_v2(&f).await;
        seed(&f).await;
        let before = data(&f).await;
        let receipt_before = receipt(&f).await;
        let legacy_before = journal(&f, false).await;
        let sqlx_before = journal(&f, true).await;
        f.pool.execute("CREATE SCHEMA phase20_fault;CREATE FUNCTION phase20_fault.fail_intent() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN IF TG_TAG='CREATE TABLE' AND current_schema()='public' AND current_query() LIKE '%CREATE TABLE work_order_reminder_intents%' THEN RAISE EXCEPTION 'controlled reminder migration failure';END IF;END $$;CREATE EVENT TRIGGER phase20_fail_intent ON ddl_command_start EXECUTE FUNCTION phase20_fault.fail_intent()")
            .await.expect("owned reminder DDL failure injector");
        let error = initialize_database(&f.pool).await.expect_err("real target reminder CREATE must fail");
        let mut source = std::error::Error::source(&error);
        let mut injected = false;
        while let Some(current) = source {
            injected |= current.to_string().contains("controlled reminder migration failure");
            source = current.source();
        }
        assert!(injected, "do not mistake an earlier inventory rejection for the injected DDL failure");
        assert_eq!(journal(&f, false).await, legacy_before);
        assert_eq!(journal(&f, true).await, sqlx_before);
        assert_eq!(data(&f).await, before);
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT row_to_json(t)::text FROM work_order_create_idempotency t").fetch_one(&f.pool).await.unwrap(), receipt_before);
        assert!(sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass('public.work_order_reminder_intents')::text").fetch_one(&f.pool).await.unwrap().is_none());
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_namespace WHERE nspname LIKE 'work_order_baseline_%'").fetch_one(&f.pool).await.unwrap(), 0);
        f.pool.execute("DROP EVENT TRIGGER phase20_fail_intent;DROP SCHEMA phase20_fault CASCADE").await.expect("remove owned reminder fault injector");
        assert!(initialize_database(&f.pool).await.is_ok());
        assert_current(&f).await;
        assert_eq!(data(&f).await, before);
    }).await;
}

#[tokio::test]
#[ignore = "requires owned CI PostgreSQL"]
async fn concurrent_v2_upgrade_and_node_rust_restart_preserve_intents() {
    owned(|f| async move {
        sqlx_v2(&f).await;
        seed(&f).await;
        let before = data(&f).await;
        let receipt_before = receipt(&f).await;
        let (a, b) = tokio::join!(initialize_database(&f.pool), initialize_database(&f.pool));
        let a = a.unwrap();
        let b = b.unwrap();
        assert!(matches!(
            (a, b),
            (
                work_orders_api::InitializationReport::Upgraded,
                work_orders_api::InitializationReport::AlreadyCurrent
            ) | (
                work_orders_api::InitializationReport::AlreadyCurrent,
                work_orders_api::InitializationReport::Upgraded
            )
        ));
        assert_current(&f).await;
        let intent_before = reminder_intent(&f).await;
        let legacy_before = journal(&f, false).await;
        let sqlx_before = journal(&f, true).await;
        let ((), rust) = tokio::join!(node_migrate(&f), initialize_database(&f.pool));
        assert_eq!(
            rust.unwrap(),
            work_orders_api::InitializationReport::AlreadyCurrent
        );
        assert_eq!(journal(&f, false).await, legacy_before);
        assert_eq!(journal(&f, true).await, sqlx_before);
        assert_eq!(data(&f).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT row_to_json(t)::text FROM work_order_create_idempotency t"
            )
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            receipt_before
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT row_to_json(t)::text FROM work_order_reminder_intents t"
            )
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            intent_before
        );
    })
    .await;
}
