use std::{collections::BTreeMap, fmt, net::IpAddr, time::Duration};

use sqlx::{
    postgres::{PgConnectOptions, PgSslMode},
    ConnectOptions,
};
use url::Url;

use crate::error::ConfigError;

/// Validated, environment-only configuration for the independent business process.
///
/// Only `WORK_ORDERS_DATABASE_URL` supplies the database identity. This loader does
/// not read environment files or accept the platform's `DATABASE_URL` as a fallback.
/// Database-only loading remains available for migration tooling. Running the
/// protected HTTP application additionally requires complete platform settings.
/// `Debug` never includes database connection options or credentials.
///
/// ```
/// use work_orders_api::Config;
/// let config = Config::from_pairs([
///     ("WORK_ORDERS_DATABASE_URL", "postgresql://worker:example@127.0.0.1/orders"),
///     ("WORK_ORDERS_HOST", "127.0.0.1"),
/// ])?;
/// assert_eq!(config.bind_address().port(), 3004);
/// # Ok::<(), work_orders_api::ConfigError>(())
/// ```
#[derive(Clone)]
pub struct Config {
    pub(crate) metrics_token: Option<String>,
    pub(crate) database: PgConnectOptions,
    pub(crate) bind: std::net::SocketAddr,
    pub(crate) acquire_timeout: Duration,
    pub(crate) statement_timeout: Duration,
    pub(crate) lock_timeout: Duration,
    pub(crate) startup_timeout: Duration,
    pub(crate) shutdown_timeout: Duration,
    pub(crate) health_timeout: Duration,
    pub(crate) max_connections: u32,
    pub(crate) platform: Option<PlatformConfig>,
    pub(crate) allowed_origins: Vec<String>,
    production: bool,
}

#[derive(Clone)]
pub(crate) struct PlatformConfig {
    pub(crate) legacy_authorization: bool,
    pub(crate) origin: Url,
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    pub(crate) timeout: Duration,
    pub(crate) max_concurrent: usize,
    pub(crate) ca_cert_file: Option<std::path::PathBuf>,
}

impl fmt::Debug for PlatformConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlatformConfig")
            .field("credentials", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .field("max_concurrent", &self.max_concurrent)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Load the current process environment, without automatic `.env` loading.
    /// Returns a sanitized error when any required value or bound is invalid.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Validate explicit configuration pairs without changing process globals.
    /// This is useful for launchers and deterministic tests. Unknown pairs are
    /// ignored; database URI query parameters are separately restricted.
    pub fn from_pairs<K, V>(pairs: impl IntoIterator<Item = (K, V)>) -> Result<Self, ConfigError>
    where
        K: Into<String>,
        V: Into<String>,
    {
        let values: BTreeMap<String, String> = pairs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        Self::from_lookup(|key| values.get(key).cloned())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let production = get("NODE_ENV").as_deref() == Some("production");
        let database =
            database_options(&get("WORK_ORDERS_DATABASE_URL").ok_or(ConfigError::DatabaseUrl)?)?;
        let host = get("WORK_ORDERS_HOST").unwrap_or_else(|| "0.0.0.0".into());
        // Binding numeric IPs avoids an unbounded DNS lookup during startup.
        let host: IpAddr = host.trim().parse().map_err(|_| ConfigError::Host)?;
        let port = number(&get, "WORK_ORDERS_PORT", 3004, u64::from(production), 65535)?;
        let millis = |name, default, min, max| {
            number(&get, name, default, min, max).map(Duration::from_millis)
        };
        let platform = platform_config(&get, production)?;
        let allowed_origins = get("WORK_ORDERS_ALLOWED_ORIGINS")
            .unwrap_or_else(|| "null".into())
            .split(',')
            .map(|raw| {
                let raw = raw.trim();
                if raw == "null" {
                    Ok(raw.to_owned())
                } else {
                    exact_origin(raw, production)
                        .map(|url| url.origin().ascii_serialization())
                        .map_err(|_| ConfigError::AllowedOrigin)
                }
            })
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?
            .into_iter()
            .collect();
        let metrics_token = get("WORK_ORDERS_METRICS_TOKEN").filter(|v| !v.is_empty());
        if metrics_token.as_ref().is_some_and(|v| {
            v.len() < 16
                || v.len() > 1024
                // The metrics endpoint reads a visible ASCII Bearer header.
                || !v.bytes().all(|byte| byte.is_ascii_graphic())
                || platform.as_ref().is_some_and(|p| p.client_secret == *v)
        }) {
            return Err(ConfigError::MetricsToken);
        }
        Ok(Self {
            metrics_token,
            database,
            bind: (host, port as u16).into(),
            acquire_timeout: millis("WORK_ORDERS_DB_ACQUIRE_TIMEOUT_MS", 5000, 100, 30000)?,
            statement_timeout: millis("WORK_ORDERS_DB_STATEMENT_TIMEOUT_MS", 5000, 100, 30000)?,
            lock_timeout: millis("WORK_ORDERS_DB_LOCK_TIMEOUT_MS", 5000, 100, 30000)?,
            startup_timeout: millis("WORK_ORDERS_STARTUP_TIMEOUT_MS", 30000, 1000, 300000)?,
            shutdown_timeout: millis("WORK_ORDERS_SHUTDOWN_TIMEOUT_MS", 10000, 100, 60000)?,
            health_timeout: millis("WORK_ORDERS_HEALTH_TIMEOUT_MS", 2000, 100, 10000)?,
            max_connections: number(&get, "WORK_ORDERS_DB_MAX_CONNECTIONS", 10, 1, 30)? as u32,
            platform,
            allowed_origins,
            production,
        })
    }

    /// Numeric address for the HTTP listener. Port zero is permitted outside
    /// production to support isolated test launches.
    pub fn bind_address(&self) -> std::net::SocketAddr {
        self.bind
    }
}

fn exact_origin(raw: &str, production: bool) -> Result<Url, ConfigError> {
    let url = Url::parse(raw).map_err(|_| ConfigError::PlatformOrigin)?;
    let development_loopback = !production
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.origin().ascii_serialization() != raw
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.scheme() != "https" && !development_loopback)
    {
        return Err(ConfigError::PlatformOrigin);
    }
    Ok(url)
}

fn platform_config(
    get: &impl Fn(&str) -> Option<String>,
    production: bool,
) -> Result<Option<PlatformConfig>, ConfigError> {
    let names = [
        "PLATFORM_API_ORIGIN",
        "PLATFORM_CLIENT_ID",
        "PLATFORM_CLIENT_SECRET",
        "PLATFORM_AUTH_TIMEOUT_MS",
        "PLATFORM_AUTH_MAX_CONCURRENT",
        "PLATFORM_AUTHORIZATION_MODE",
        "PLATFORM_CA_CERT_FILE",
    ];
    if !names.iter().any(|name| get(name).is_some()) {
        return Ok(None);
    }
    let origin = exact_origin(
        get("PLATFORM_API_ORIGIN").as_deref().unwrap_or("").trim(),
        production,
    )?;
    let client_id = get("PLATFORM_CLIENT_ID")
        .unwrap_or_default()
        .trim()
        .to_owned();
    if client_id.is_empty()
        || client_id.len() > 100
        || !client_id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"._-".contains(&ch))
    {
        return Err(ConfigError::PlatformClientId);
    }
    let client_secret = get("PLATFORM_CLIENT_SECRET").unwrap_or_default();
    if client_secret.is_empty()
        || client_secret.len() > 1024
        || client_secret.contains(['\r', '\n'])
    {
        return Err(ConfigError::PlatformClientSecret);
    }
    let legacy_authorization = match get("PLATFORM_AUTHORIZATION_MODE")
        .as_deref()
        .unwrap_or("combined")
    {
        "combined" => false,
        "legacy" => true,
        _ => return Err(ConfigError::AuthorizationMode),
    };
    Ok(Some(PlatformConfig {
        legacy_authorization,
        origin,
        client_id,
        client_secret,
        timeout: Duration::from_millis(number(get, "PLATFORM_AUTH_TIMEOUT_MS", 3000, 100, 10000)?),
        max_concurrent: number(get, "PLATFORM_AUTH_MAX_CONCURRENT", 32, 1, 128)? as usize,
        ca_cert_file: get("PLATFORM_CA_CERT_FILE")
            .map(|raw| {
                if raw.trim().is_empty() || raw.contains('\0') {
                    Err(ConfigError::PlatformCa)
                } else {
                    Ok(std::path::PathBuf::from(raw.trim()))
                }
            })
            .transpose()?,
    }))
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("production", &self.production)
            .field("database", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

fn number(
    get: &impl Fn(&str) -> Option<String>,
    name: &'static str,
    fallback: u64,
    min: u64,
    max: u64,
) -> Result<u64, ConfigError> {
    let raw = get(name).unwrap_or_else(|| fallback.to_string());
    if raw.is_empty() || !raw.bytes().all(|ch| ch.is_ascii_digit()) {
        return Err(ConfigError::Number(name));
    }
    let value: u64 = raw.parse().map_err(|_| ConfigError::Number(name))?;
    if !(min..=max).contains(&value) {
        return Err(ConfigError::Number(name));
    }
    Ok(value)
}

fn database_options(raw: &str) -> Result<PgConnectOptions, ConfigError> {
    let mut url = Url::parse(raw.trim()).map_err(|_| ConfigError::DatabaseUrl)?;
    if !matches!(url.scheme(), "postgres" | "postgresql")
        || url.host_str().is_none_or(str::is_empty)
        || url.username().is_empty()
        || url.path().trim_matches('/').is_empty()
        || url.path().trim_start_matches('/').contains('/')
        || url.fragment().is_some()
    {
        return Err(ConfigError::DatabaseUrl);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut mode = None;
    // SQLx logs unrecognized URI parameter values. Reject these before invoking
    // its parser so a typo can never print a credential or arbitrary URI value.
    for (key, value) in url.query_pairs() {
        let category = match key.as_ref() {
            "sslmode" | "ssl-mode" => {
                mode = Some(
                    value
                        .parse::<PgSslMode>()
                        .map_err(|_| ConfigError::DatabaseTls)?,
                );
                "sslmode"
            }
            "sslrootcert" | "ssl-root-cert" | "ssl-ca" => "sslrootcert",
            "sslcert" | "ssl-cert" => "sslcert",
            "sslkey" | "ssl-key" => "sslkey",
            "application_name" => "application_name",
            _ => return Err(ConfigError::DatabaseUrl),
        };
        if !seen.insert(category) || value.is_empty() {
            return Err(ConfigError::DatabaseUrl);
        }
    }
    // PostgreSQL transport policy is independent from the platform HTTPS policy.
    // Preserve explicit libpq SSL modes and the driver's Prefer default. Internal
    // private-network PostgreSQL is allowed; verify-full retains normal certificate
    // and hostname verification when selected by the deployment.
    let mode = mode.unwrap_or(PgSslMode::Prefer);
    // Pin a deterministic policy even if the launching shell contains PGSSLMODE.
    if !seen.contains("sslmode") {
        url.query_pairs_mut().append_pair("sslmode", "prefer");
    }
    let options: PgConnectOptions = url.as_str().parse().map_err(|_| ConfigError::DatabaseUrl)?;
    Ok(options
        .ssl_mode(mode)
        .application_name("work-orders-api")
        .disable_statement_logging())
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "postgresql://worker:TOP_SECRET@127.0.0.1/orders";

    fn config(extra: &[(&str, &str)]) -> Result<Config, ConfigError> {
        Config::from_pairs(
            std::iter::once(("WORK_ORDERS_DATABASE_URL", URL)).chain(extra.iter().copied()),
        )
    }
    #[test]
    fn private_metrics_are_opt_in_bounded_independent_and_redacted() {
        assert!(config(&[]).unwrap().metrics_token.is_none());
        let cfg = config(&[("WORK_ORDERS_METRICS_TOKEN", "private-operations-canary")]).unwrap();
        assert!(cfg.metrics_token.is_some());
        assert!(!format!("{cfg:?}").contains("private-operations-canary"));
        for invalid in [
            "short",
            "contains whitespace secret",
            "不可用于请求头的指标令牌",
            "private\0operations-canary",
            "private\u{7f}operations-canary",
        ] {
            assert_eq!(
                config(&[("WORK_ORDERS_METRICS_TOKEN", invalid)]).unwrap_err(),
                ConfigError::MetricsToken
            );
        }
        assert_eq!(
            config(&[
                ("PLATFORM_API_ORIGIN", "https://private.example.test"),
                ("PLATFORM_CLIENT_ID", "work-client"),
                ("PLATFORM_CLIENT_SECRET", "private-operations-canary"),
                ("WORK_ORDERS_METRICS_TOKEN", "private-operations-canary")
            ])
            .unwrap_err(),
            ConfigError::MetricsToken
        );
    }

    #[test]
    fn explicit_business_url_only_and_no_platform_credentials() {
        assert!(Config::from_pairs([("DATABASE_URL", URL)]).is_err());
        let config = config(&[]).unwrap();
        assert_eq!(config.bind.port(), 3004);
        assert_eq!(config.acquire_timeout, Duration::from_millis(5000));
        assert!(!format!("{config:?}").contains("TOP_SECRET"));
        assert!(!format!("{config:?}").contains("worker"));
    }

    #[test]
    fn validates_all_numeric_budgets_and_bind_address() {
        for name in [
            "WORK_ORDERS_PORT",
            "WORK_ORDERS_DB_ACQUIRE_TIMEOUT_MS",
            "WORK_ORDERS_DB_STATEMENT_TIMEOUT_MS",
            "WORK_ORDERS_DB_LOCK_TIMEOUT_MS",
            "WORK_ORDERS_STARTUP_TIMEOUT_MS",
            "WORK_ORDERS_SHUTDOWN_TIMEOUT_MS",
            "WORK_ORDERS_HEALTH_TIMEOUT_MS",
            "WORK_ORDERS_DB_MAX_CONNECTIONS",
        ] {
            for value in ["", "-1", " 10", "1.5", "NaN", "999999999999999999999999"] {
                assert!(config(&[(name, value)]).is_err(), "{name}={value}");
            }
        }
        for host in ["localhost", "https://example.test", "127.0.0.1/", ""] {
            assert!(config(&[("WORK_ORDERS_HOST", host)]).is_err());
        }
        assert!(config(&[("WORK_ORDERS_HOST", "::1")]).is_ok());
        assert!(config(&[("WORK_ORDERS_PORT", "0")]).is_ok());
        assert!(config(&[("WORK_ORDERS_PORT", "0"), ("NODE_ENV", "production")]).is_err());
    }

    #[test]
    fn database_tls_policy_is_explicit_and_independent_from_production_http() {
        let production = config(&[("NODE_ENV", "production")]).unwrap();
        assert!(matches!(
            production.database.get_ssl_mode(),
            PgSslMode::Prefer
        ));
        for mode in [
            "disable",
            "allow",
            "prefer",
            "require",
            "verify-ca",
            "verify-full",
        ] {
            let uri = format!("{URL}?sslmode={mode}");
            let config = Config::from_pairs([
                ("WORK_ORDERS_DATABASE_URL", uri.as_str()),
                ("NODE_ENV", "production"),
            ])
            .unwrap();
            assert_eq!(
                format!("{:?}", config.database.get_ssl_mode()),
                format!("{:?}", mode.parse::<PgSslMode>().unwrap())
            );
        }
        assert!(Config::from_pairs([
            (
                "WORK_ORDERS_DATABASE_URL",
                format!("{URL}?sslmode=verify-full")
            ),
            ("NODE_ENV", "production".into())
        ])
        .is_ok());
    }

    #[test]
    fn rejects_malformed_or_ambiguous_database_urls_without_echoing_them() {
        for uri in [
            "postgresql:///missing_host",
            "postgresql://127.0.0.1/missing_user",
            "postgresql://worker@127.0.0.1/",
            "https://worker:TOP_SECRET@example.test/orders",
            "postgresql://worker@127.0.0.1/orders?typo=TOP_SECRET",
            "postgresql://worker@127.0.0.1/orders?options=-c%20search_path=evil",
            "postgresql://worker@127.0.0.1/orders?sslmode=prefer&ssl-mode=disable",
        ] {
            let error = Config::from_pairs([("WORK_ORDERS_DATABASE_URL", uri)]).unwrap_err();
            assert!(!format!("{error} {error:?}").contains("TOP_SECRET"));
        }
    }

    #[test]
    fn platform_settings_are_optional_only_as_a_complete_absence() {
        assert!(config(&[]).unwrap().platform.is_none());
        for name in [
            "PLATFORM_API_ORIGIN",
            "PLATFORM_CLIENT_ID",
            "PLATFORM_CLIENT_SECRET",
            "PLATFORM_AUTH_TIMEOUT_MS",
            "PLATFORM_AUTH_MAX_CONCURRENT",
            "PLATFORM_AUTHORIZATION_MODE",
            "PLATFORM_CA_CERT_FILE",
        ] {
            assert!(config(&[(name, "some-value")]).is_err(), "{name}");
        }
        let complete = [
            ("PLATFORM_API_ORIGIN", "https://platform.example.test"),
            ("PLATFORM_CLIENT_ID", "work-orders-client"),
            ("PLATFORM_CLIENT_SECRET", "PRIVATE_CLIENT_SECRET"),
        ];
        let cfg = config(&complete).unwrap();
        let settings = cfg.platform.as_ref().unwrap();
        assert_eq!(settings.timeout, Duration::from_millis(3000));
        assert_eq!(settings.max_concurrent, 32);
        assert!(!settings.legacy_authorization);
        for (mode, legacy) in [("combined", false), ("legacy", true)] {
            let mut values = complete.to_vec();
            values.push(("PLATFORM_AUTHORIZATION_MODE", mode));
            assert_eq!(
                config(&values)
                    .unwrap()
                    .platform
                    .unwrap()
                    .legacy_authorization,
                legacy
            );
        }
        assert_eq!(cfg.allowed_origins, ["null"]);
        assert!(!format!("{cfg:?} {settings:?}").contains("PRIVATE_CLIENT_SECRET"));
        for (name, value) in [
            ("PLATFORM_CLIENT_ID", "bad:client"),
            ("PLATFORM_CLIENT_SECRET", ""),
            ("PLATFORM_AUTHORIZATION_MODE", "automatic"),
            ("PLATFORM_CLIENT_SECRET", "private\r\nvalue"),
            ("PLATFORM_AUTH_TIMEOUT_MS", "99"),
            ("PLATFORM_AUTH_TIMEOUT_MS", "10001"),
            ("PLATFORM_AUTH_MAX_CONCURRENT", "0"),
            ("PLATFORM_AUTH_MAX_CONCURRENT", "129"),
            ("PLATFORM_CA_CERT_FILE", ""),
        ] {
            let mut values = complete.to_vec();
            values.push((name, value));
            let error = config(&values).unwrap_err();
            assert!(!format!("{error} {error:?}").contains("PRIVATE_CLIENT_SECRET"));
        }
    }

    #[test]
    fn origins_require_exact_https_or_development_loopback_http() {
        for origin in [
            "https://platform.test",
            "https://platform.test:8443",
            "http://127.0.0.1:3000",
            "http://localhost:3000",
            "http://[::1]:3000",
        ] {
            assert!(exact_origin(origin, false).is_ok(), "{origin}");
        }
        for origin in [
            "http://internal.test",
            "https://platform.test/",
            "https://platform.test/path",
            "https://platform.test?x=1",
            "https://platform.test#frag",
            "https://user:secret@platform.test",
            "https://platform.test:443",
            "https://PLATFORM.test",
            "null",
            "",
        ] {
            assert!(exact_origin(origin, false).is_err(), "{origin}");
        }
        assert!(exact_origin("http://127.0.0.1:3000", true).is_err());
        assert!(exact_origin("https://platform.test", true).is_ok());
        let allowed = config(&[(
            "WORK_ORDERS_ALLOWED_ORIGINS",
            "null,https://frontend.test,null",
        )])
        .unwrap();
        assert_eq!(allowed.allowed_origins, ["https://frontend.test", "null"]);
        for origins in [
            "",
            "null,",
            "https://frontend.test/",
            "http://internal.test",
        ] {
            assert!(config(&[("WORK_ORDERS_ALLOWED_ORIGINS", origins)]).is_err());
        }
    }
}
