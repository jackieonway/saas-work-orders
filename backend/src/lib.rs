//! Independent work-order service baseline.
//!
//! The executable owns only its business PostgreSQL pool. It never reads the
//! platform database, automatically loads `.env` files, or invokes Node.js.
//! Strict schema initialization finishes before the HTTP listener is created.
//! This phase exposes process liveness and business database/schema readiness;
//! authenticated read and export routes preserve the platform's per-request
//! introspection/context boundary. Writes preserve transactional idempotency and
//! version checks. Member lookup and assignment use the platform's controlled
//! direct-member API before business transactions. Reminders durably bind their
//! event intent before one authorized platform publication attempt.

mod authorization;
mod config;
mod directory;
mod error;
mod http;
mod migrations;
mod reads;
mod reminders;
mod writes;

use std::{future::IntoFuture, time::Duration};

use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::{net::TcpListener, sync::oneshot};

use error::ServiceFailure;

pub use config::Config;
pub use error::{ConfigError, ServiceError};
pub use http::{application, router};
pub use migrations::{
    MigrationError as InitializationError, MigrationOutcome as InitializationReport,
};

/// Initialize or strictly verify the independent business database baseline.
///
/// This coordinates concurrent legacy/Rust starts, checks the immutable legacy
/// SHA-256 migration and exact schema, and records SQLx adoption transactionally.
/// Existing work orders and counters are never rewritten. Any inconsistent
/// state is rejected; no HTTP listener is opened by this function.
pub async fn initialize_database(
    pool: &PgPool,
) -> Result<InitializationReport, InitializationError> {
    migrations::initialize(pool).await
}

/// Open the independent pool with bounded acquisition and server-side statement,
/// lock and idle transaction limits. This does not execute migrations or claim
/// readiness; call [`initialize_database`] before constructing [`router`].
pub async fn connect_database(config: &Config) -> Result<PgPool, ServiceError> {
    let statement = format!("{}ms", config.statement_timeout.as_millis());
    let lock = format!("{}ms", config.lock_timeout.as_millis());
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(config.acquire_timeout)
        .idle_timeout(Some(Duration::from_secs(30)))
        .after_connect(move |connection, _| {
            let statement = statement.clone();
            let lock = lock.clone();
            Box::pin(async move {
                sqlx::query(
                    "SELECT pg_catalog.set_config('statement_timeout',$1,false), \
                     pg_catalog.set_config('lock_timeout',$2,false), \
                     pg_catalog.set_config('idle_in_transaction_session_timeout','15000ms',false), \
                     pg_catalog.set_config('search_path','pg_catalog,public,pg_temp',false)",
                )
                .bind(statement)
                .bind(lock)
                .execute(connection)
                .await?;
                Ok(())
            })
        })
        .connect_with(config.database.clone())
        .await
        .map_err(|error| ServiceError::caused_by(ServiceFailure::DatabaseUnavailable, error))
}

/// Run the standalone service until SIGINT/SIGTERM, enforcing an absolute startup
/// budget before binding and a separate bounded graceful shutdown budget.
/// All returned lifecycle errors contain sanitized categories only.
pub async fn run(config: Config) -> Result<(), ServiceError> {
    let (pool, outcome, authorizer) = tokio::time::timeout(config.startup_timeout, async {
        let authorizer = authorization::Authorizer::new(&config)?;
        let pool = connect_database(&config).await?;
        match initialize_database(&pool).await {
            Ok(outcome) => Ok((pool, outcome, authorizer)),
            Err(error) => {
                tracing::error!(event = "work_orders_baseline_rejected", reason = %error);
                pool.close().await;
                Err(ServiceError::caused_by(
                    ServiceFailure::InitializationFailed,
                    error,
                ))
            }
        }
    })
    .await
    .map_err(|error| ServiceError::caused_by(ServiceFailure::StartupTimedOut, error))??;
    tracing::info!(event = "work_orders_database_initialized", outcome = ?outcome);

    let listener = match TcpListener::bind(config.bind).await {
        Ok(listener) => listener,
        Err(error) => {
            close_pool(&pool, config.shutdown_timeout).await?;
            return Err(ServiceError::caused_by(
                ServiceFailure::ListenerFailed,
                error,
            ));
        }
    };
    let address = listener
        .local_addr()
        .map_err(|error| ServiceError::caused_by(ServiceFailure::ListenerFailed, error))?;
    tracing::info!(event = "work_orders_baseline_listening", host = %address.ip(), port = address.port(), scope = "business-database-schema");

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let app = http::configured_metrics_router(
        pool.clone(),
        config.health_timeout,
        Some(authorizer),
        config.allowed_origins.clone(),
        config.metrics_token.clone(),
    );
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => {
            close_pool(&pool, config.shutdown_timeout).await?;
            result.map_err(|error| ServiceError::caused_by(ServiceFailure::ListenerFailed, error))
        }
        signal = shutdown_signal() => {
            // Even a signal registration failure must close the already-open listener.
            let _ = shutdown_tx.send(());
            tokio::time::timeout(config.shutdown_timeout, async {
                let result = server.await;
                pool.close().await;
                signal?;
                result.map_err(|error| ServiceError::caused_by(ServiceFailure::ListenerFailed, error))
            }).await.map_err(|error| ServiceError::caused_by(ServiceFailure::ShutdownTimedOut, error))?
        }
    }
}

async fn close_pool(pool: &PgPool, timeout: Duration) -> Result<(), ServiceError> {
    tokio::time::timeout(timeout, pool.close())
        .await
        .map_err(|error| ServiceError::caused_by(ServiceFailure::ShutdownTimedOut, error))
}

async fn shutdown_signal() -> Result<(), ServiceError> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = signal(SignalKind::terminate())
            .map_err(|error| ServiceError::caused_by(ServiceFailure::SignalFailed, error))?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(|error| ServiceError::caused_by(ServiceFailure::SignalFailed, error)),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| ServiceError::caused_by(ServiceFailure::SignalFailed, error))
}

mod telemetry;
