use std::{error::Error, fmt};

/// A sanitized configuration failure. No variant contains a supplied value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The selected database initialization mode is unsupported.
    DatabaseInitMode,
    /// Offline import preparation requires the migration-only executable.
    ImportRequiresMigrationCommand,
    /// Imported startup requires a complete, canonical trusted receipt.
    ImportReceipt,
    /// The configured database URL does not identify the receipt's target.
    ImportTarget,
    /// Invalid private metrics credential.
    MetricsToken,
    /// The selected authorization protocol is unsupported.
    AuthorizationMode,
    /// The external canonical identity issuer is not an exact HTTPS origin.
    PlatformIdentityIssuer,
    /// Resource-to-OAuth scope bindings are malformed or ambiguous.
    ResourceScopeBindings,
    /// The independent PostgreSQL URL is missing, ambiguous, or unsupported.
    DatabaseUrl,
    /// Database TLS configuration contains an unsupported SSL mode.
    DatabaseTls,
    /// The HTTP bind host is not a numeric IPv4 or IPv6 address.
    Host,
    /// The platform endpoint is not an exact trusted HTTPS origin.
    PlatformOrigin,
    /// The application client identity is missing or malformed.
    PlatformClientId,
    /// The application client secret is missing or malformed.
    PlatformClientSecret,
    /// An allowed browser origin is missing or malformed.
    AllowedOrigin,
    /// The additional platform CA file is missing or invalid.
    PlatformCa,
    /// A named numeric option is outside its documented bounds.
    Number(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatabaseInitMode => f.write_str("WORK_ORDERS_DATABASE_INIT_MODE must be seed, import or imported"),
            Self::ImportRequiresMigrationCommand => f.write_str("IMPORT_REQUIRES_MIGRATE_ONLY: use work-orders-migrate --import"),
            Self::ImportReceipt => f.write_str("IMPORTED_DATABASE_RECEIPT_INVALID: expected database, import marker and schema SHA256 are required only in imported mode"),
            Self::ImportTarget => f.write_str("IMPORTED_DATABASE_TARGET_MISMATCH: database URL must match WORK_ORDERS_EXPECTED_DATABASE"),
            Self::MetricsToken => f.write_str("metrics token must be a bounded non-whitespace credential"),
            Self::AuthorizationMode => f.write_str("PLATFORM_AUTHORIZATION_MODE must be canonical or absent"),
            Self::PlatformIdentityIssuer => f.write_str("PLATFORM_IDENTITY_ISSUER must be an exact HTTPS issuer origin"),
            Self::ResourceScopeBindings => f.write_str("PLATFORM_RESOURCE_SCOPE_BINDINGS must map known operations to unique explicit OAuth scopes"),
            Self::DatabaseUrl => f.write_str("WORK_ORDERS_DATABASE_URL must explicitly identify the independent PostgreSQL database"),
            Self::DatabaseTls => f.write_str("database TLS configuration contains an unsupported SSL mode"),
            Self::Host => f.write_str("WORK_ORDERS_HOST must be a numeric bind address"),
            Self::PlatformOrigin => f.write_str("PLATFORM_API_ORIGIN must be an exact HTTPS origin (development permits loopback HTTP)"),
            Self::PlatformClientId => f.write_str("PLATFORM_CLIENT_ID must identify the application APP_RESOURCE service"),
            Self::PlatformClientSecret => f.write_str("PLATFORM_CLIENT_SECRET must contain a valid confidential client secret"),
            Self::AllowedOrigin => f.write_str("WORK_ORDERS_ALLOWED_ORIGINS must contain exact HTTP(S) host origins; opaque null and wildcards are forbidden"),
            Self::PlatformCa => f.write_str("PLATFORM_CA_CERT_FILE must identify a valid PEM certificate file"),
            Self::Number(name) => write!(f, "{name} must be an integer within its documented bounds"),
        }
    }
}

impl Error for ConfigError {}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ServiceFailure {
    PlatformConfiguration,
    PlatformClient,
    DatabaseUnavailable,
    InitializationFailed,
    StartupTimedOut,
    ListenerFailed,
    SignalFailed,
    ShutdownTimedOut,
}

/// A sanitized lifecycle failure. `Display` and `Debug` expose only a fixed
/// operational category. The private source chain is retained for controlled
/// diagnostics and must never be written to public HTTP responses or logs.
pub struct ServiceError {
    failure: ServiceFailure,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl ServiceError {
    pub(crate) fn caused_by(
        failure: ServiceFailure,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            failure,
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Debug for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceError")
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.failure {
            ServiceFailure::PlatformConfiguration => {
                "platform authorization configuration is required"
            }
            ServiceFailure::PlatformClient => "platform authorization client initialization failed",
            ServiceFailure::DatabaseUnavailable => "business database connection unavailable",
            ServiceFailure::InitializationFailed => {
                "business database baseline verification failed"
            }
            ServiceFailure::StartupTimedOut => "business database startup budget exhausted",
            ServiceFailure::ListenerFailed => "HTTP listener unavailable",
            ServiceFailure::SignalFailed => "shutdown signal registration failed",
            ServiceFailure::ShutdownTimedOut => "graceful shutdown budget exhausted",
        })
    }
}

/// An API failure consisting exclusively of fixed, safe protocol text.
/// The status is independent of the HTTP framework; adapters own serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ApiError {
    status: u16,
    code: &'static str,
    message: &'static str,
}

impl ApiError {
    pub(crate) const fn new(status: u16, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }

    pub(crate) const fn status(self) -> u16 {
        self.status
    }
    pub(crate) const fn code(self) -> &'static str {
        self.code
    }
    pub(crate) const fn message(self) -> &'static str {
        self.message
    }

    pub(crate) const fn invalid_token() -> Self {
        Self::new(401, "INVALID_APP_TOKEN", "应用令牌无效或平台会话已失效")
    }

    pub(crate) const fn invalid_context() -> Self {
        Self::new(401, "INVALID_CONTEXT", "平台授权上下文失效或身份不一致")
    }

    pub(crate) const fn data_denied() -> Self {
        Self::new(403, "DATA_DENIED", "缺少有效的独立数据授权")
    }

    pub(crate) const fn platform_unavailable() -> Self {
        Self::new(
            503,
            "PLATFORM_UNAVAILABLE",
            "平台授权服务暂时不可用，请稍后重试",
        )
    }

    pub(crate) const fn validation() -> Self {
        Self::new(400, "VALIDATION_ERROR", "请求参数无效")
    }

    pub(crate) const fn not_found() -> Self {
        Self::new(404, "RESOURCE_NOT_FOUND", "工单不存在或不在当前数据范围内")
    }

    pub(crate) const fn database_unavailable() -> Self {
        Self::new(503, "DATABASE_UNAVAILABLE", "工单业务数据库暂时不可用")
    }

    pub(crate) const fn export_limit() -> Self {
        Self::new(413, "EXPORT_LIMIT", "CSV 超过 4 MiB，请缩小筛选范围")
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl Error for ApiError {}

impl Error for ServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_error_preserves_source_but_never_formats_driver_detail() {
        let error = ServiceError::caused_by(
            ServiceFailure::DatabaseUnavailable,
            std::io::Error::other("postgresql://worker:private-password@private-host/orders"),
        );
        assert!(error.source().is_some());
        for rendered in [format!("{error}"), format!("{error:?}")] {
            for private in ["postgresql", "worker", "private-password", "private-host"] {
                assert!(!rendered.contains(private));
            }
        }
    }
}
