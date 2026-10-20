//! Atomic adoption of the independently owned, immutable work-order baseline.

mod catalog;

use std::{borrow::Cow, error::Error, fmt};

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{
    migrate::{Migrate, Migration, MigrationType},
    PgConnection, PgPool,
};

const BASELINE_SQL: &str = include_str!("../migrations/0001_work_orders.sql");
const IDEMPOTENCY_SQL: &str = include_str!("../migrations/0002_create_idempotency.sql");
const REMINDER_SQL: &str = include_str!("../migrations/0003_reminder_intents.sql");
const LEGACY_VERSION: &str = "0001_work_orders.sql";
const IDEMPOTENCY_VERSION: &str = "0002_create_idempotency.sql";
const REMINDER_VERSION: &str = "0003_reminder_intents.sql";
// This checksum pins the historical bytes, including their original line endings.
const LEGACY_SHA256: &str = "a2e4224febd195fdd663a8954c3be4b76a8a06338521056c6809ebaa57d04090";
const SQLX_VERSION: i64 = 1;
const SQLX_DESCRIPTION: &str = "work orders";
const LEGACY_TABLE_SQL: &str = "CREATE TABLE work_order_migrations(version TEXT PRIMARY KEY,checksum CHAR(64) NOT NULL,applied_at TIMESTAMPTZ NOT NULL DEFAULT now())";

/// The verified baseline transition completed by this startup.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationOutcome {
    /// The dedicated database was empty and received both migration histories.
    Fresh,
    /// The exact historical schema and SHA-256 history were adopted without replay.
    AdoptedLegacy,
    /// An exact prior Rust baseline was advanced without rewriting its history.
    Upgraded,
    /// Both histories and the complete baseline schema already matched.
    AlreadyCurrent,
}

#[derive(Clone, Copy, Debug)]
enum Failure {
    EmbeddedBaseline,
    UnsupportedDatabase,
    LegacyHistory,
    SqlxHistory,
    SchemaDrift,
    Database,
}

impl Failure {
    fn message(self) -> &'static str {
        match self {
            Self::EmbeddedBaseline => "the embedded historical migration checksum is invalid",
            Self::UnsupportedDatabase => {
                "the database is not a supported dedicated work-order database"
            }
            Self::LegacyHistory => "the historical work-order migration history does not match",
            Self::SqlxHistory => "the SQLx work-order migration history does not match",
            Self::SchemaDrift => {
                "the work-order database schema does not match the immutable baseline"
            }
            Self::Database => "the work-order baseline database operation failed",
        }
    }
}

/// A baseline failure whose `Display` and `Debug` never expose SQL or connection details.
///
/// No schema or migration-history repair is attempted. The private source chain is
/// retained for controlled diagnostics and must not be written to public logs.
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
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.failure.message())
    }
}

impl fmt::Debug for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MigrationError")
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl Error for MigrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

impl From<sqlx::Error> for MigrationError {
    fn from(source: sqlx::Error) -> Self {
        Self {
            failure: Failure::Database,
            source: Some(Box::new(source)),
        }
    }
}

impl From<sqlx::migrate::MigrateError> for MigrationError {
    fn from(source: sqlx::migrate::MigrateError) -> Self {
        Self {
            failure: Failure::Database,
            source: Some(Box::new(source)),
        }
    }
}

fn baseline() -> Result<Migration, MigrationError> {
    let checksum = Sha256::digest(BASELINE_SQL.as_bytes());
    let expected = LEGACY_SHA256.as_bytes();
    let alphabet = b"0123456789abcdef";
    if checksum.iter().enumerate().any(|(index, byte)| {
        expected[index * 2] != alphabet[usize::from(byte >> 4)]
            || expected[index * 2 + 1] != alphabet[usize::from(byte & 15)]
    }) {
        return Err(MigrationError::invalid(Failure::EmbeddedBaseline));
    }
    Ok(Migration::new(
        SQLX_VERSION,
        Cow::Borrowed(SQLX_DESCRIPTION),
        MigrationType::Simple,
        Cow::Borrowed(BASELINE_SQL),
        false,
    ))
}

fn migrations() -> Result<[Migration; 3], MigrationError> {
    Ok([
        baseline()?,
        Migration::new(
            2,
            Cow::Borrowed("create idempotency"),
            MigrationType::Simple,
            Cow::Borrowed(IDEMPOTENCY_SQL),
            false,
        ),
        Migration::new(
            3,
            Cow::Borrowed("reminder intents"),
            MigrationType::Simple,
            Cow::Borrowed(REMINDER_SQL),
            false,
        ),
    ])
}

fn legacy_checksum(migration: &Migration) -> String {
    format!("{:x}", Sha256::digest(migration.sql.as_bytes()))
}

/// Initialize, adopt or upgrade the pinned sequence, preserving all business rows.
///
/// The caller must bound startup time and use an explicitly selected, independently
/// owned business database. The migration role needs `CREATE` on that database to
/// construct a transaction-local reference schema. Any validation error rolls back
/// the reference schema and both history changes before HTTP readiness can open.
pub(crate) async fn initialize(pool: &PgPool) -> Result<MigrationOutcome, MigrationError> {
    let migrations = migrations()?;
    let mut transaction = pool.begin().await?;
    // This is exactly the lock taken by the preserved Node migration runner.
    sqlx::query("SELECT pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended('work-orders-business-migrations',11))")
        .execute(&mut *transaction)
        .await?;
    set_schema(&mut transaction, "public").await?;
    let state = catalog::inventory(&mut transaction).await?;

    // Only internally generated, ASCII identifier bytes enter DDL identifiers.
    let reference = format!("work_order_baseline_{}", uuid::Uuid::new_v4().simple());
    execute_batch(&mut transaction, &format!("CREATE SCHEMA {reference}")).await?;
    set_schema(&mut transaction, &reference).await?;
    execute_batch(&mut transaction, LEGACY_TABLE_SQL).await?;
    execute_batch(&mut transaction, BASELINE_SQL).await?;
    transaction.ensure_migrations_table().await?;
    let reference_legacy_v1 = catalog::snapshot(&mut transaction, &reference, false).await?;
    let reference_current_v1 = catalog::snapshot(&mut transaction, &reference, true).await?;
    execute_batch(&mut transaction, IDEMPOTENCY_SQL).await?;
    let reference_legacy_v2 = catalog::snapshot(&mut transaction, &reference, false).await?;
    let reference_current_v2 = catalog::snapshot(&mut transaction, &reference, true).await?;
    execute_batch(&mut transaction, REMINDER_SQL).await?;
    let reference_legacy_v3 = catalog::snapshot(&mut transaction, &reference, false).await?;
    let reference_current_v3 = catalog::snapshot(&mut transaction, &reference, true).await?;
    set_schema(&mut transaction, "public").await?;

    let (outcome, legacy_count, sqlx_count) = match state {
        catalog::Inventory::Empty => {
            execute_batch(&mut transaction, LEGACY_TABLE_SQL).await?;
            execute_batch(&mut transaction, BASELINE_SQL).await?;
            record_legacy(&mut transaction, &migrations[0]).await?;
            (MigrationOutcome::Fresh, 1, 0)
        }
        catalog::Inventory::Existing {
            sqlx,
            idempotency,
            reminders,
        } => {
            lock_existing(&mut transaction, sqlx, idempotency, reminders).await?;
            let legacy_count = validate_legacy(&mut transaction, &migrations).await?;
            let sqlx_count = if sqlx {
                validate_sqlx(&mut transaction, &migrations).await?
            } else {
                0
            };
            // Node may append 0002/0003 while SQLx has an older prefix. SQLx ahead of
            // the compatibility ledger, or a table ahead of its history, is not
            // a legitimate transition produced by either supported runner.
            let schema_version = if reminders {
                3
            } else if idempotency {
                2
            } else {
                1
            };
            if legacy_count != schema_version || sqlx_count > legacy_count {
                return Err(MigrationError::invalid(Failure::LegacyHistory));
            }
            let reference = match (sqlx, idempotency, reminders) {
                (false, false, false) => &reference_legacy_v1,
                (true, false, false) => &reference_current_v1,
                (false, true, false) => &reference_legacy_v2,
                (true, true, false) => &reference_current_v2,
                (false, true, true) => &reference_legacy_v3,
                (true, true, true) => &reference_current_v3,
                (_, false, true) => return Err(MigrationError::invalid(Failure::SchemaDrift)),
            };
            if &catalog::snapshot(&mut transaction, "public", sqlx).await? != reference {
                return Err(MigrationError::invalid(Failure::SchemaDrift));
            }
            let outcome = if !sqlx {
                MigrationOutcome::AdoptedLegacy
            } else if sqlx_count < migrations.len() {
                MigrationOutcome::Upgraded
            } else {
                MigrationOutcome::AlreadyCurrent
            };
            (outcome, legacy_count, sqlx_count)
        }
    };

    for migration in migrations.iter().skip(legacy_count) {
        execute_batch(&mut transaction, &migration.sql).await?;
        record_legacy(&mut transaction, migration).await?;
    }
    transaction.ensure_migrations_table().await?;
    for migration in migrations.iter().skip(sqlx_count) {
        record_sqlx(&mut transaction, migration).await?;
    }
    if validate_legacy(&mut transaction, &migrations).await? != migrations.len()
        || validate_sqlx(&mut transaction, &migrations).await? != migrations.len()
        || catalog::snapshot(&mut transaction, "public", true).await? != reference_current_v3
    {
        return Err(MigrationError::invalid(Failure::SchemaDrift));
    }
    execute_batch(
        &mut transaction,
        &format!("DROP SCHEMA {reference} CASCADE"),
    )
    .await?;
    transaction.commit().await?;
    Ok(outcome)
}

async fn execute_batch(connection: &mut PgConnection, sql: &str) -> Result<(), MigrationError> {
    // Executor's boxed future keeps this startup API Send on SQLx 0.8; using
    // RawSql::execute's generic async wrapper does not preserve that property.
    sqlx::Executor::execute(connection, sql).await?;
    Ok(())
}

async fn lock_existing(
    connection: &mut PgConnection,
    sqlx: bool,
    idempotency: bool,
    reminders: bool,
) -> Result<(), MigrationError> {
    let mut tables = Vec::with_capacity(5);
    if idempotency {
        // Match create's receipt-before-counter/order locking direction.
        tables.push("public.work_order_create_idempotency");
    }
    tables.extend(["public.work_order_counters", "public.work_orders"]);
    // Reminder claims take a ticket SHARE lock before inserting the intent.
    // Preserve that order, as well as create's receipt/counter/ticket order.
    if reminders {
        tables.push("public.work_order_reminder_intents");
    }
    tables.push("public.work_order_migrations");
    if sqlx {
        tables.push("public._sqlx_migrations");
    }
    let statement = format!("LOCK TABLE {} IN ACCESS EXCLUSIVE MODE", tables.join(", "));
    sqlx::query(&statement).execute(connection).await?;
    Ok(())
}

async fn set_schema(connection: &mut PgConnection, schema: &str) -> Result<(), MigrationError> {
    // pg_catalog remains implicit-first; putting pg_temp last prevents relation
    // shadowing while unqualified historical CREATE still targets this schema.
    sqlx::query("SELECT pg_catalog.set_config('search_path',pg_catalog.quote_ident($1::text)||',pg_temp',true)")
        .bind(schema)
        .execute(connection)
        .await?;
    Ok(())
}

async fn record_legacy(
    connection: &mut PgConnection,
    migration: &Migration,
) -> Result<(), MigrationError> {
    let version = legacy_version(migration.version)?;
    sqlx::query("INSERT INTO public.work_order_migrations(version,checksum) VALUES ($1,$2)")
        .bind(version)
        .bind(legacy_checksum(migration))
        .execute(connection)
        .await?;
    Ok(())
}

async fn record_sqlx(
    connection: &mut PgConnection,
    migration: &Migration,
) -> Result<(), MigrationError> {
    // SQLx's own Migration computes SHA-384; adoption never replays known DDL.
    sqlx::query("INSERT INTO public._sqlx_migrations(version,description,success,checksum,execution_time) VALUES ($1,$2,true,$3,0)")
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .execute(connection)
        .await?;
    Ok(())
}

async fn validate_legacy(
    connection: &mut PgConnection,
    migrations: &[Migration],
) -> Result<usize, MigrationError> {
    let rows: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT version,checksum,applied_at IS NOT NULL FROM public.work_order_migrations ORDER BY version LIMIT 4",
    )
    .fetch_all(connection)
    .await?;
    if rows.is_empty()
        || rows.len() > migrations.len()
        || rows.iter().zip(migrations).any(|(row, migration)| {
            legacy_version(migration.version).ok() != Some(row.0.as_str())
                || row.1 != legacy_checksum(migration)
                || !row.2
        })
    {
        return Err(MigrationError::invalid(Failure::LegacyHistory));
    }
    Ok(rows.len())
}

async fn validate_sqlx(
    connection: &mut PgConnection,
    migrations: &[Migration],
) -> Result<usize, MigrationError> {
    let rows: Vec<(i64, String, bool, Vec<u8>, i64, bool)> = sqlx::query_as(
        "SELECT version,description,success,checksum,execution_time,installed_on IS NOT NULL FROM public._sqlx_migrations ORDER BY version LIMIT 4",
    )
    .fetch_all(connection)
    .await?;
    if rows.is_empty()
        || rows.len() > migrations.len()
        || rows.iter().zip(migrations).any(|(row, migration)| {
            row.0 != migration.version
        || row.1 != migration.description
        || !row.2
        || row.3 != migration.checksum.as_ref()
        // SQLx itself may leave -1 when disconnected before timing bookkeeping.
        || row.4 < -1
        || !row.5
        })
    {
        return Err(MigrationError::invalid(Failure::SqlxHistory));
    }
    Ok(rows.len())
}

fn legacy_version(version: i64) -> Result<&'static str, MigrationError> {
    match version {
        1 => Ok(LEGACY_VERSION),
        2 => Ok(IDEMPOTENCY_VERSION),
        3 => Ok(REMINDER_VERSION),
        _ => Err(MigrationError::invalid(Failure::LegacyHistory)),
    }
}

/// Read-only readiness after the strict startup proof has succeeded.
///
/// This checks reachability, exact histories, required relations, and an enabled
/// immutable-identity trigger. It deliberately does not create a reference schema
/// per probe or claim to repeat startup's complete catalog comparison.
pub(crate) async fn verify_database(pool: &PgPool) -> Result<(), MigrationError> {
    let migrations = migrations()?;
    let mut connection = pool.acquire().await?;
    if validate_legacy(&mut connection, &migrations).await? != migrations.len()
        || validate_sqlx(&mut connection, &migrations).await? != migrations.len()
    {
        return Err(MigrationError::invalid(Failure::LegacyHistory));
    }
    let usable: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid=t.tgrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_proc p ON p.oid=t.tgfoid JOIN pg_catalog.pg_namespace pn ON pn.oid=p.pronamespace WHERE n.nspname='public' AND c.relname='work_orders' AND t.tgname='work_order_identity_immutable' AND NOT t.tgisinternal AND t.tgenabled='O' AND pn.nspname='public' AND p.proname='work_order_identity_immutable') AND pg_catalog.to_regclass('public.work_order_counters') IS NOT NULL",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !usable {
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
    fn historical_bytes_and_sqlx_checksum_formats_stay_distinct() {
        let migration = baseline().expect("immutable checked-in baseline");
        assert_eq!(LEGACY_SHA256.len(), 64);
        assert_eq!(migration.checksum.len(), 48);
        assert_eq!(migration.version, 1);
        assert!(!migration.no_tx);
    }

    #[test]
    fn error_formatting_never_exposes_private_database_detail() {
        let error = MigrationError::from(sqlx::Error::Protocol(
            "postgres://secret@private-host/business?password=hidden".to_owned(),
        ));
        assert_eq!(
            error.to_string(),
            "the work-order baseline database operation failed"
        );
        assert!(!format!("{error:?}").contains("secret"));
        assert!(error.source().is_some());
    }

    #[test]
    fn initialization_future_supports_tokio_worker_tasks() {
        fn accepts_send(_: impl std::future::Future + Send) {}
        fn compile_only(pool: &PgPool) {
            accepts_send(initialize(pool));
        }
        let _: fn(&PgPool) = compile_only;
    }
}
