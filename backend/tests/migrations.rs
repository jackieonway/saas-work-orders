//! Real, self-owned PostgreSQL migration and adoption regressions.
//! Run explicitly through the owned CI PostgreSQL driver; ordinary unit tests ignore these.
use sqlx::{postgres::PgPoolOptions, Executor, PgPool};
use std::{future::Future, time::Duration};
use uuid::Uuid;
use work_orders_api::initialize_database;

#[derive(Clone)]
struct Fixture {
    pool: PgPool,
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
    let local = std::env::var("WORK_ORDERS_TEST_LOCAL_POSTGRES_OWNER").ok();
    let owned = match local.as_deref() {
        None => url.host_str() == Some(format!("{owner}-postgres").as_str()),
        Some(value) => {
            matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
                && url.port().is_some_and(|p| p >= 1024 && p != 5432)
                && value
                    == format!(
                        "{owner}@{}:{}",
                        url.host_str().unwrap(),
                        url.port().unwrap()
                    )
        }
    };
    assert!(owned, "explicit test database ownership required");
    assert_eq!(url.path(), "/postgres");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&raw)
        .await
        .expect("owned admin connection");
    let name = format!("work_orders_phase16_{}", Uuid::now_v7().simple());
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
        tenant: Uuid::now_v7(),
        ticket: Uuid::now_v7(),
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

#[tokio::test]
#[ignore = "requires explicitly owned PostgreSQL"]
async fn fresh_baseline_restarts_and_rejects_history_or_schema_drift() {
    owned(|f| async move {
        initialize_database(&f.pool).await.unwrap();
        initialize_database(&f.pool).await.unwrap();
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(versions, vec![1]);
        let old: bool =
            sqlx::query_scalar("SELECT to_regclass('public.work_order_migrations') IS NOT NULL")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert!(!old);
        sqlx::query("ALTER TABLE work_orders ADD COLUMN unexpected text")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(initialize_database(&f.pool).await.is_err());
        sqlx::query("ALTER TABLE work_orders DROP COLUMN unexpected")
            .execute(&f.pool)
            .await
            .unwrap();
        initialize_database(&f.pool).await.unwrap();
        sqlx::query(
            "UPDATE _sqlx_migrations SET checksum=decode(repeat('00',48),'hex') WHERE version=1",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        assert!(initialize_database(&f.pool).await.is_err());
    })
    .await;
}
#[tokio::test]
#[ignore = "requires explicitly owned PostgreSQL"]
async fn fresh_baseline_enforces_uuid7_and_immutable_work_order_identity() {
    owned(|f|async move{
  initialize_database(&f.pool).await.unwrap();
  let org=Uuid::now_v7();let user=Uuid::now_v7();
  sqlx::query("INSERT INTO work_orders(id,tenant_id,organization_id,owner_user_id,organization_name,owner_display_name,number,title) VALUES($1,$2,$3,$4,'Fresh organization','Fresh owner','FRESH-001','Fresh work order')").bind(f.ticket).bind(f.tenant).bind(org).bind(user).execute(&f.pool).await.unwrap();
  assert!(sqlx::query("UPDATE work_orders SET tenant_id=$2 WHERE id=$1").bind(f.ticket).bind(Uuid::now_v7()).execute(&f.pool).await.is_err());
  let old=Uuid::parse_str("019d0000-0000-4000-8000-000000000001").unwrap();
  assert!(sqlx::query("INSERT INTO work_orders(id,tenant_id,organization_id,owner_user_id,organization_name,owner_display_name,number,title) VALUES($1,$2,$3,$4,'Fresh organization','Fresh owner','FRESH-002','Rejected UUID')").bind(old).bind(f.tenant).bind(org).bind(user).execute(&f.pool).await.is_err());
 }).await;
}

#[tokio::test]
#[ignore = "requires explicitly owned PostgreSQL"]
async fn historical_or_untracked_database_is_rejected_without_adoption() {
    owned(|f| async move {
        sqlx::query("CREATE TABLE work_order_migrations(version text PRIMARY KEY)")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(initialize_database(&f.pool).await.is_err());
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM work_order_migrations")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
        let initialized: bool =
            sqlx::query_scalar("SELECT to_regclass('public.work_orders') IS NOT NULL")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert!(!initialized);
    })
    .await;
}
