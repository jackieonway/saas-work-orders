//! Database-only command runner. Argument and environment validation finishes
//! before a pool can be opened; HTTP/platform configuration is never loaded.

use std::{collections::BTreeSet, error::Error, ffi::OsString, fmt, time::Duration};

use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
    ConnectOptions,
};
use url::Url;

use crate::{
    error::{ServiceError, ServiceFailure},
    ConfigError,
};

/// A fixed-category migration command failure.
///
/// Supplied arguments, database URLs and driver details are never included in
/// `Display` or `Debug`. Driver causes are retained only for controlled diagnostics.
#[derive(Debug)]
pub enum MigrationCommandError {
    /// An unknown, positional, non-Unicode or repeated argument was supplied.
    InvalidArguments,
    /// Seed and import modes were requested together.
    ConflictingModes,
    /// This service has no seed operation.
    SeedUnsupported,
    /// A PG-prefixed environment variable could supply hidden driver settings.
    AmbientDatabaseEnvironment,
    /// The dedicated database URL is missing or invalid.
    Configuration(ConfigError),
    /// Connection, initialization or bounded shutdown failed.
    Database(ServiceError),
}

impl fmt::Display for MigrationCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArguments => formatter.write_str(
                "invalid migration arguments; only a single --import is supported",
            ),
            Self::ConflictingModes => formatter.write_str("--seed and --import are mutually exclusive"),
            Self::SeedUnsupported => formatter.write_str("--seed is not supported; this service has no seed data"),
            Self::AmbientDatabaseEnvironment => formatter.write_str(
                "PG-prefixed environment variables are not supported; configure only WORK_ORDERS_DATABASE_URL",
            ),
            Self::Configuration(error) => fmt::Display::fmt(error, formatter),
            Self::Database(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl Error for MigrationCommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

/// Apply and verify the service's existing migrations without starting HTTP.
///
/// `args` excludes the executable name. No arguments and a single `--import`
/// both initialize the already-created database; `--import` does not import rows.
/// `--seed` is rejected, and combining it with `--import` is explicitly rejected.
/// Unknown or repeated arguments fail before configuration or connection.
///
/// Only `WORK_ORDERS_DATABASE_URL` supplies connection settings. This function
/// loads no environment files, requires no platform/CORS configuration, creates
/// no database or role, and rejects PG-prefixed ambient driver settings.
/// Default pgpass files are ignored. Explicit local postgres peer URLs may
/// provide an absolute socket `host` and a plain `options[role]` owner name.
/// Connections have a five-second acquisition budget; initialization is bounded
/// to five minutes and pool shutdown to ten seconds. The pool is closed after
/// every completed or timed-out initialization attempt.
pub async fn run_migration_command(
    args: impl IntoIterator<Item = OsString>,
) -> Result<(), MigrationCommandError> {
    validate_args(args)?;
    let database = database_from_lookup(
        |name| std::env::var(name).ok(),
        std::env::vars_os().map(|(name, _)| name),
    )?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query(
                    "SELECT pg_catalog.set_config('statement_timeout','30000ms',false), \
                     pg_catalog.set_config('lock_timeout','5000ms',false), \
                     pg_catalog.set_config('idle_in_transaction_session_timeout','15000ms',false), \
                     pg_catalog.set_config('search_path','pg_catalog,public,pg_temp',false)",
                )
                .execute(connection)
                .await?;
                Ok(())
            })
        })
        .connect_with(database)
        .await
        .map_err(|error| database_error(ServiceFailure::DatabaseUnavailable, error))?;
    let initialized =
        tokio::time::timeout(Duration::from_secs(300), crate::initialize_database(&pool)).await;
    // Do not return early from initialization errors: close the same pool first.
    let closed = tokio::time::timeout(Duration::from_secs(10), pool.close()).await;
    initialized
        .map_err(|error| database_error(ServiceFailure::StartupTimedOut, error))?
        .map_err(|error| database_error(ServiceFailure::InitializationFailed, error))?;
    closed.map_err(|error| database_error(ServiceFailure::ShutdownTimedOut, error))?;
    Ok(())
}

fn database_error(
    failure: ServiceFailure,
    source: impl Error + Send + Sync + 'static,
) -> MigrationCommandError {
    MigrationCommandError::Database(ServiceError::caused_by(failure, source))
}

fn validate_args(args: impl IntoIterator<Item = OsString>) -> Result<(), MigrationCommandError> {
    let mut import = false;
    let mut seed = false;
    for argument in args {
        match argument.to_str() {
            Some("--import") if !import => import = true,
            Some("--seed") if !seed => seed = true,
            _ => return Err(MigrationCommandError::InvalidArguments),
        }
    }
    if import && seed {
        return Err(MigrationCommandError::ConflictingModes);
    }
    if seed {
        return Err(MigrationCommandError::SeedUnsupported);
    }
    Ok(())
}

fn database_from_lookup(
    get: impl Fn(&str) -> Option<String>,
    environment_keys: impl IntoIterator<Item = OsString>,
) -> Result<PgConnectOptions, MigrationCommandError> {
    // SQLx defaults consult PG* variables before applying a URL. Refuse hidden
    // credentials, transport and session options instead of mutating process env.
    if environment_keys.into_iter().any(|name| {
        name.to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("PG")
    }) {
        return Err(MigrationCommandError::AmbientDatabaseEnvironment);
    }
    let invalid = || MigrationCommandError::Configuration(ConfigError::DatabaseUrl);
    let raw = get("WORK_ORDERS_DATABASE_URL").ok_or_else(invalid)?;
    let url = Url::parse(raw.trim()).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "postgres" | "postgresql")
        || url.host_str().is_none_or(str::is_empty)
        || url.username().is_empty()
        || url.path().trim_matches('/').is_empty()
        || url.path().trim_start_matches('/').contains('/')
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(invalid());
    }
    let username = decode_component(url.username())?;
    let database = decode_component(url.path().trim_start_matches('/'))?;
    // Unlike FromStr, this constructor never reads the default pgpass file.
    // Every required identity field and the port are supplied explicitly below.
    let mut options = PgConnectOptions::new_without_pgpass()
        .host(url.host_str().ok_or_else(invalid)?)
        .port(url.port().unwrap_or(5432))
        .username(&username)
        .database(&database)
        .ssl_mode(PgSslMode::Prefer)
        .application_name("work-orders-migrate")
        .disable_statement_logging();
    if let Some(password) = url.password() {
        options = options.password(&decode_component(password)?);
    }
    let mut seen = BTreeSet::new();
    let mut socket = None;
    let mut role = None;
    for (key, value) in url.query_pairs() {
        let category = match key.as_ref() {
            "sslmode" | "ssl-mode" => "sslmode",
            "sslrootcert" | "ssl-root-cert" | "ssl-ca" => "sslrootcert",
            "sslcert" | "ssl-cert" => "sslcert",
            "sslkey" | "ssl-key" => "sslkey",
            "application_name" => "application_name",
            "host" => "host",
            "options[role]" => "role",
            _ => return Err(invalid()),
        };
        if !seen.insert(category) || value.is_empty() || value.chars().any(char::is_control) {
            return Err(invalid());
        }
        options = match category {
            "sslmode" => options.ssl_mode(
                value
                    .parse()
                    .map_err(|_| MigrationCommandError::Configuration(ConfigError::DatabaseTls))?,
            ),
            "sslrootcert" => options.ssl_root_cert(value.as_ref()),
            "sslcert" => options.ssl_client_cert(value.as_ref()),
            "sslkey" => options.ssl_client_key(value.as_ref()),
            "application_name" => options,
            "host" if socket_directory(&value) => {
                socket = Some(value.into_owned());
                options
            }
            "role" if role_identifier(&value) => {
                role = Some(value.into_owned());
                options
            }
            _ => return Err(invalid()),
        };
    }
    if let Some(role) = role {
        // Delegated NOLOGIN ownership is only supported over an explicit local
        // peer socket as postgres; never accept arbitrary startup GUC options.
        if username != "postgres"
            || url.host_str() != Some("localhost")
            || url.password().is_some()
            || socket.is_none()
        {
            return Err(invalid());
        }
        options = options.options([("role", role)]);
    }
    if let Some(socket) = socket {
        options = options.socket(socket);
    }
    Ok(options)
}

fn decode_component(raw: &str) -> Result<String, MigrationCommandError> {
    let invalid = || MigrationCommandError::Configuration(ConfigError::DatabaseUrl);
    let mut decoded = Vec::with_capacity(raw.len());
    let mut bytes = raw.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next().and_then(|byte| char::from(byte).to_digit(16));
            let low = bytes.next().and_then(|byte| char::from(byte).to_digit(16));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(invalid());
            };
            decoded.push((high * 16 + low) as u8);
        } else {
            decoded.push(byte);
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| invalid())?;
    if decoded.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(decoded)
}

fn socket_directory(value: &str) -> bool {
    value.starts_with('/')
        && value.len() > 1
        && !value.contains('\\')
        && value
            .split('/')
            .skip(1)
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn role_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && !value.as_bytes()[0].is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn migration_arguments_have_no_implicit_seed_or_import_operation() {
        assert!(validate_args(args(&[])).is_ok());
        assert!(validate_args(args(&["--import"])).is_ok());
        assert!(matches!(
            validate_args(args(&["--seed"])),
            Err(MigrationCommandError::SeedUnsupported)
        ));
        for values in [["--seed", "--import"], ["--import", "--seed"]] {
            assert!(matches!(
                validate_args(args(&values)),
                Err(MigrationCommandError::ConflictingModes)
            ));
        }
        for values in [
            vec!["--import", "--import"],
            vec!["--seed", "--seed"],
            vec!["--import=true"],
            vec!["--help"],
            vec!["--"],
            vec!["postgresql://private-user:private-password@private-host/private-db"],
        ] {
            let error = validate_args(args(&values)).unwrap_err();
            assert!(matches!(error, MigrationCommandError::InvalidArguments));
            assert!(!format!("{error} {error:?}").contains("private-"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_arguments_are_rejected_without_panicking() {
        use std::os::unix::ffi::OsStringExt;
        assert!(matches!(
            validate_args([OsString::from_vec(vec![0xff])]),
            Err(MigrationCommandError::InvalidArguments)
        ));
    }

    #[test]
    fn database_configuration_reads_only_the_dedicated_url_and_pins_the_port() {
        for (url, port) in [
            (
                "postgresql://migration_worker:private-password@127.0.0.1/new_database",
                5432,
            ),
            (
                "postgresql://migration_worker:private-password@127.0.0.1:55439/new_database",
                55439,
            ),
        ] {
            let database = database_from_lookup(
                |name| {
                    assert_eq!(name, "WORK_ORDERS_DATABASE_URL");
                    Some(url.into())
                },
                [],
            )
            .unwrap();
            assert_eq!(database.get_host(), "127.0.0.1");
            assert_eq!(database.get_port(), port);
            assert_eq!(database.get_username(), "migration_worker");
            assert_eq!(database.get_database(), Some("new_database"));
        }
        assert!(matches!(
            database_from_lookup(
                |name| {
                    assert_eq!(name, "WORK_ORDERS_DATABASE_URL");
                    None
                },
                [],
            ),
            Err(MigrationCommandError::Configuration(
                ConfigError::DatabaseUrl
            ))
        ));
    }

    #[test]
    fn local_peer_delegation_preserves_the_socket_and_exact_startup_role() {
        let raw = "postgresql://postgres@localhost:5432/new_database?host=/run/postgresql&options[role]=new_database_owner";
        let options = database_from_lookup(|_| Some(raw.into()), []).unwrap();
        assert_eq!(options.get_username(), "postgres");
        assert_eq!(options.get_database(), Some("new_database"));
        assert_eq!(options.get_host(), "localhost");
        assert_eq!(options.get_port(), 5432);
        assert_eq!(
            options.get_socket().unwrap().to_str(),
            Some("/run/postgresql")
        );
        assert_eq!(options.get_options(), Some("-c role=new_database_owner"));
        assert!(options.to_url_lossy().password().is_none());
        for query in [
            "host=relative/socket",
            "host=/run/../postgresql",
            "host=/run//postgresql",
            "host=/run/postgresql/",
            "host=127.0.0.1",
            "options[role]=new_database_owner",
            "host=/run/postgresql&options[role]=owner%20-c%20search_path%3Devil",
            "host=/run/postgresql&options[role]=%22quoted_owner%22",
            "host=/run/postgresql&options[role]=1owner",
            "host=/run/postgresql&options[role]=",
            "host=/run/postgresql&options[role]=owner&options[role]=other",
            "host=/run/postgresql&host=/tmp",
            "host=/run/postgresql&options[search_path]=evil",
            "host=/run/postgresql&options=-c%20role%3Downer",
            "hostaddr=127.0.0.1",
            "user=other",
            "dbname=other",
            "password=private-password",
            "sslmode=disable&ssl-mode=prefer",
        ] {
            let raw = format!("postgresql://postgres@localhost:5432/new_database?{query}");
            assert!(database_from_lookup(|_| Some(raw.clone()), []).is_err());
        }
        for raw in [
            "postgresql://other@localhost/new_database?host=/run/postgresql&options[role]=owner",
            "postgresql://postgres:private-password@localhost/new_database?host=/run/postgresql&options[role]=owner",
            "postgresql://postgres@127.0.0.1/new_database?host=/run/postgresql&options[role]=owner",
        ] {
            assert!(database_from_lookup(|_| Some(raw.into()), []).is_err());
        }
    }

    #[test]
    fn url_components_are_decoded_without_driver_fallback_or_query_logging() {
        let options = database_from_lookup(
            |_| Some("postgresql://worker%2Bname:p%40ss%3A%25%2B@127.0.0.1/new%5Fdatabase?ssl-mode=verify-full".into()),
            [],
        )
        .unwrap();
        assert_eq!(options.get_username(), "worker+name");
        assert_eq!(options.get_database(), Some("new_database"));
        assert!(matches!(options.get_ssl_mode(), PgSslMode::VerifyFull));
        assert_eq!(
            decode_component(options.to_url_lossy().password().unwrap()).unwrap(),
            "p@ss:%+"
        );
        for raw in [
            "postgresql://worker%00@127.0.0.1/new_database",
            "postgresql://worker:invalid%GG@127.0.0.1/new_database",
            "postgresql://worker@127.0.0.1/new%FFdatabase",
            "postgresql://worker@127.0.0.1:0/new_database",
        ] {
            assert!(database_from_lookup(|_| Some(raw.into()), []).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_pgpass_is_never_a_migration_credential_source() {
        use std::{os::unix::fs::PermissionsExt, process::Command};

        const PROBE: &str = "MIGRATION_COMMAND_PGPASS_TEST_CHILD";
        const RAW: &str = "postgresql://migration_worker@127.0.0.1/new_database";
        if std::env::var_os(PROBE).is_some() {
            // Control: the ordinary driver parser does read this same fixture.
            let ordinary: PgConnectOptions = RAW.parse().unwrap();
            assert_eq!(
                decode_component(ordinary.to_url_lossy().password().unwrap()).unwrap(),
                "private-pgpass-canary"
            );
            let migration = database_from_lookup(|_| Some(RAW.into()), []).unwrap();
            assert!(migration.to_url_lossy().password().is_none());
            return;
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "migration-no-pgpass-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.0.join(".pgpass"));
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let passfile = directory.join(".pgpass");
        std::fs::write(
            &passfile,
            "127.0.0.1:5432:new_database:migration_worker:private-pgpass-canary\n",
        )
        .unwrap();
        std::fs::set_permissions(&passfile, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        for (name, _) in std::env::vars_os() {
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("PG")
            {
                command.env_remove(name);
            }
        }
        let output = command
            .args([
                "--exact",
                "migration_cli::tests::default_pgpass_is_never_a_migration_credential_source",
            ])
            .env(PROBE, "1")
            .env("HOME", &directory)
            .output()
            .unwrap();
        assert!(output.status.success(), "isolated pgpass regression failed");
    }

    #[test]
    fn ambient_driver_settings_and_invalid_urls_fail_with_sanitized_categories() {
        for key in ["PGPORT", "PGPASSWORD", "PGOPTIONS", "PGPASSFILE", "pgport"] {
            let result = database_from_lookup(
                |_| panic!("ambient settings must fail before looking up the URL"),
                [OsString::from(key)],
            );
            assert!(matches!(
                result,
                Err(MigrationCommandError::AmbientDatabaseEnvironment)
            ));
        }
        for raw in [
            "private-invalid-url",
            "postgresql://private-user:private-password@127.0.0.1/private-db?options=private-options",
            "postgresql://private-user:private-password@127.0.0.1/private-db?sslmode=private-mode",
        ] {
            let error = database_from_lookup(|_| Some(raw.into()), []).unwrap_err();
            assert!(!format!("{error} {error:?}").contains("private-"));
        }
        let error = database_error(
            ServiceFailure::DatabaseUnavailable,
            std::io::Error::other(
                "postgresql://private-user:private-password@private-host/private-db",
            ),
        );
        assert!(error.source().is_some());
        assert!(!format!("{error} {error:?}").contains("private-"));
    }
}
