//! Fresh, single-history work-order initialization; historical adoption is removed.
use serde::Serialize;
use sqlx::{
    migrate::{Migration, MigrationType, Migrator},
    Connection, PgConnection, PgPool,
};
use std::{borrow::Cow, error::Error, fmt};
const BASELINE_SQL: &str = include_str!("../migrations/0001_work_orders.sql");
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationOutcome {
    Fresh,
    AlreadyCurrent,
}
#[derive(Clone, Copy, Debug)]
enum Failure {
    Database,
    SchemaDrift,
    SqlxHistory,
}
#[derive(Debug)]
pub struct MigrationError {
    failure: Failure,
    source: Option<Box<dyn Error + Send + Sync>>,
}
impl MigrationError {
    fn invalid(failure: Failure) -> Self {
        Self {
            failure,
            source: None,
        }
    }
}
impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.failure {
            Failure::Database => "work-order initialization failed",
            Failure::SchemaDrift => "work-order schema drift detected",
            Failure::SqlxHistory => "work-order migration history rejected; recreate the database",
        })
    }
}
impl Error for MigrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|e| e as &(dyn Error + 'static))
    }
}
impl From<sqlx::Error> for MigrationError {
    fn from(error: sqlx::Error) -> Self {
        Self {
            failure: Failure::Database,
            source: Some(Box::new(error)),
        }
    }
}
impl From<sqlx::migrate::MigrateError> for MigrationError {
    fn from(error: sqlx::migrate::MigrateError) -> Self {
        Self {
            failure: Failure::Database,
            source: Some(Box::new(error)),
        }
    }
}
fn migrator() -> Migrator {
    Migrator {
        migrations: Cow::Owned(vec![Migration::new(
            1,
            Cow::Borrowed("work orders"),
            MigrationType::Simple,
            Cow::Borrowed(BASELINE_SQL),
            false,
        )]),
        ..Migrator::DEFAULT
    }
}
pub(crate) async fn initialize(pool: &PgPool) -> Result<MigrationOutcome, MigrationError> {
    let mut connection = pool.acquire().await?.detach();
    let initialized: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut connection)
            .await?;
    let result=async{
  if !initialized {
   let occupied:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','S'))").fetch_one(&mut connection).await?;
   if occupied{return Err(MigrationError::invalid(Failure::SchemaDrift));}
  }
  sqlx::query("SELECT set_config('search_path','public,pg_catalog,pg_temp',false)").execute(&mut connection).await?;
  migrator().run_direct(&mut connection).await?;
  verify_connection(&mut connection).await
 }.await;
    let closed = connection.close().await;
    result?;
    closed?;
    Ok(if initialized {
        MigrationOutcome::AlreadyCurrent
    } else {
        MigrationOutcome::Fresh
    })
}
pub(crate) async fn verify_database(pool: &PgPool) -> Result<(), MigrationError> {
    let mut connection = pool.acquire().await?;
    verify_connection(&mut connection).await
}
pub(crate) async fn verify_connection(connection: &mut PgConnection) -> Result<(), MigrationError> {
    let expected = migrator();
    let rows: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT version,success,checksum FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *connection)
    .await?;
    if rows.len() != 1
        || rows[0].0 != 1
        || !rows[0].1
        || rows[0].2.as_slice() != expected.iter().next().unwrap().checksum.as_ref()
    {
        return Err(MigrationError::invalid(Failure::SqlxHistory));
    }
    let fingerprint:bool=sqlx::query_scalar("SELECT count(*)=1 AND COALESCE(bool_and(fingerprint=public.work_orders_schema_fingerprint()),false) FROM public._work_orders_schema_contract WHERE singleton").fetch_one(&mut *connection).await?;
    if !fingerprint {
        return Err(MigrationError::invalid(Failure::SchemaDrift));
    }
    let usable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid=t.tgrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_proc p ON p.oid=t.tgfoid JOIN pg_catalog.pg_namespace pn ON pn.oid=p.pronamespace WHERE n.nspname='public' AND c.relname='work_orders' AND t.tgname='work_order_identity_immutable' AND NOT t.tgisinternal AND t.tgenabled='O' AND pn.nspname='public' AND p.proname='work_order_identity_immutable') AND pg_catalog.to_regclass('public.work_order_counters') IS NOT NULL",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !usable {
        return Err(MigrationError::invalid(Failure::SchemaDrift));
    }
    let uuid_guards: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_class t ON t.oid=c.conrelid JOIN pg_catalog.pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname='public' AND c.contype='c' AND c.convalidated AND c.conname LIKE '%\\_uuid\\_v7' ESCAPE '\\'",
    ).fetch_one(&mut *connection).await?;
    if uuid_guards != 17 {
        return Err(MigrationError::invalid(Failure::SchemaDrift));
    }
    // Referencing the real required columns also detects dropped/replaced tables.
    sqlx::query("SELECT id,number,tenant_id,organization_id,owner_user_id,version FROM public.work_orders LIMIT 0")
        .execute(&mut *connection)
        .await?;
    sqlx::query("SELECT tenant_id,next_number FROM public.work_order_counters LIMIT 0")
        .execute(&mut *connection)
        .await?;
    sqlx::query("SELECT tenant_id,organization_id,user_id,action,idempotency_key,request_hash,ticket_id,response_json,created_at FROM public.work_order_create_idempotency LIMIT 0")
        .execute(&mut *connection)
        .await?;
    sqlx::query("SELECT tenant_id,organization_id,application_id,user_id,idempotency_key,ticket_id,target_organization_id,recipient_user_id,request_hash,request_json,created_at FROM public.work_order_reminder_intents LIMIT 0")
        .execute(&mut *connection)
        .await?;
    let intent_immutable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid=t.tgrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_proc p ON p.oid=t.tgfoid JOIN pg_catalog.pg_namespace pn ON pn.oid=p.pronamespace WHERE n.nspname='public' AND c.relname='work_order_reminder_intents' AND t.tgname='work_order_reminder_intent_immutable' AND NOT t.tgisinternal AND t.tgenabled='O' AND pn.nspname='public' AND p.proname='work_order_reminder_intent_immutable')",
    ).fetch_one(&mut *connection).await?;
    if !intent_immutable {
        return Err(MigrationError::invalid(Failure::SchemaDrift));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_fresh_uuid7_baseline_is_embedded() {
        let m = migrator();
        assert_eq!(m.iter().count(), 1);
        assert!(BASELINE_SQL.contains("work_orders_id_uuid_v7"));
        assert!(!BASELINE_SQL.contains("work_order_migrations"));
    }
}
