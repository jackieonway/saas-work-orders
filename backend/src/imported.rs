//! Read-only startup proof for a database imported by the offline UUID workflow.
//! Runtime configuration supplies the trusted receipt; the database cannot choose it.

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgConnectOptions, PgPool};
use uuid::{Uuid, Variant};

use crate::{
    error::{ConfigError, ServiceError, ServiceFailure},
    migrations,
};

const MARKER_PREFIX: &str = "saas-uuid-import/1/";
// Match the authenticated U04 catalog representation. In particular, public
// composite/array names must stay qualified in format_type and expressions.
const PROOF_SESSION_SETTINGS: &str = "SET LOCAL search_path = pg_catalog;\nSET LOCAL timezone = 'UTC';\nSET LOCAL row_security = off;";

#[derive(Clone)]
pub(crate) enum Initialization {
    Seed,
    Imported(Receipt),
}

#[derive(Clone)]
pub(crate) struct Receipt {
    database: String,
    marker: String,
    schema_sha256: String,
}

impl Initialization {
    pub(crate) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mode = get("WORK_ORDERS_DATABASE_INIT_MODE");
        let database = get("WORK_ORDERS_EXPECTED_DATABASE");
        let marker = get("WORK_ORDERS_IMPORT_MARKER");
        let schema_sha256 = get("WORK_ORDERS_EXPECTED_SCHEMA_SHA256");
        match mode.as_deref() {
            None | Some("seed") => {
                if database.is_some() || marker.is_some() || schema_sha256.is_some() {
                    return Err(ConfigError::ImportReceipt);
                }
                Ok(Self::Seed)
            }
            Some("import") => Err(ConfigError::ImportRequiresMigrationCommand),
            Some("imported") => {
                let (Some(database), Some(marker), Some(schema_sha256)) =
                    (database, marker, schema_sha256)
                else {
                    return Err(ConfigError::ImportReceipt);
                };
                if !valid_database(&database)
                    || !valid_marker(&marker)
                    || !valid_digest(&schema_sha256)
                {
                    return Err(ConfigError::ImportReceipt);
                }
                Ok(Self::Imported(Receipt {
                    database,
                    marker,
                    schema_sha256,
                }))
            }
            Some(_) => Err(ConfigError::DatabaseInitMode),
        }
    }

    pub(crate) fn validate_target(&self, options: &PgConnectOptions) -> Result<(), ConfigError> {
        if let Self::Imported(receipt) = self {
            if options.get_database() != Some(receipt.database.as_str())
                || options.get_username().is_empty()
                || options.get_username().chars().any(char::is_control)
                || options
                    .get_database()
                    .is_some_and(|name| name.chars().any(char::is_control))
            {
                return Err(ConfigError::ImportTarget);
            }
        }
        Ok(())
    }
}

fn valid_database(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && !value.chars().any(char::is_control)
        && !matches!(value, "postgres" | "template0" | "template1")
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_marker(value: &str) -> bool {
    let Some((batch, digest)) = value
        .strip_prefix(MARKER_PREFIX)
        .and_then(|tail| tail.split_once('/'))
    else {
        return false;
    };
    Uuid::parse_str(batch).is_ok_and(|id| {
        id.get_version_num() == 7 && id.get_variant() == Variant::RFC4122 && id.to_string() == batch
    }) && valid_digest(digest)
}

fn failure() -> ServiceError {
    ServiceError::caused_by(
        ServiceFailure::InitializationFailed,
        std::io::Error::other(
            "imported database receipt, runtime role or schema verification failed",
        ),
    )
}

fn caused_by(error: impl std::error::Error + Send + Sync + 'static) -> ServiceError {
    ServiceError::caused_by(ServiceFailure::InitializationFailed, error)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    database: String,
    comment: Option<String>,
    current_user: String,
    session_user: String,
    login: bool,
    privileged: bool,
    database_create: bool,
    database_temporary: bool,
    schema_create: bool,
    owner_access: bool,
    read_only: bool,
    isolation: String,
}

fn validate_proof(proof: &Proof, receipt: &Receipt, username: &str) -> Result<(), ServiceError> {
    if proof.database != receipt.database
        || proof.comment.as_deref() != Some(receipt.marker.as_str())
        || proof.current_user != username
        || proof.session_user != username
        || !proof.login
        || proof.privileged
        || proof.database_create
        || proof.database_temporary
        || proof.schema_create
        || proof.owner_access
        || !proof.read_only
        || proof.isolation != "repeatable read"
    {
        return Err(failure());
    }
    Ok(())
}

const ADMISSION: &str = r#"
WITH me AS (
    SELECT oid,rolcanlogin,rolsuper,rolcreatedb,rolcreaterole,rolreplication,rolbypassrls
    FROM pg_catalog.pg_roles WHERE rolname=current_user
), db AS (
    SELECT oid,datname,datdba FROM pg_catalog.pg_database WHERE datname=current_database()
), user_schemas AS (
    SELECT oid,nspowner FROM pg_catalog.pg_namespace
    WHERE nspname <> 'information_schema' AND pg_catalog.left(nspname::text,3) <> 'pg_'
), reachable_roles AS (
    SELECT r.oid FROM pg_catalog.pg_roles r CROSS JOIN me
    WHERE pg_catalog.pg_has_role(me.oid,r.oid,'SET'::text) OR pg_catalog.pg_has_role(me.oid,r.oid,'USAGE'::text)
), protected_roles AS (
    SELECT datdba AS oid FROM db
    UNION SELECT nspowner FROM user_schemas
    UNION SELECT relowner FROM pg_catalog.pg_class WHERE relnamespace IN (SELECT oid FROM user_schemas)
    UNION SELECT proowner FROM pg_catalog.pg_proc WHERE pronamespace IN (SELECT oid FROM user_schemas)
    UNION SELECT typowner FROM pg_catalog.pg_type WHERE typnamespace IN (SELECT oid FROM user_schemas)
    UNION SELECT oid FROM pg_catalog.pg_roles WHERE rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls
)
SELECT db.datname::text,
    pg_catalog.shobj_description(db.oid,'pg_database'::pg_catalog.name),
    current_user::text,session_user::text,
    me.rolcanlogin,
    me.rolsuper OR me.rolcreatedb OR me.rolcreaterole OR me.rolreplication OR me.rolbypassrls,
    EXISTS(SELECT 1 FROM reachable_roles r WHERE pg_catalog.has_database_privilege(r.oid,db.oid,'CREATE'::text)),
    EXISTS(SELECT 1 FROM reachable_roles r WHERE pg_catalog.has_database_privilege(r.oid,db.oid,'TEMPORARY'::text)),
    EXISTS(SELECT 1 FROM reachable_roles r CROSS JOIN pg_catalog.pg_namespace n
        WHERE n.nspname::text OPERATOR(pg_catalog.!~) '^pg_(toast_)?temp_'::text
          AND pg_catalog.has_schema_privilege(r.oid,n.oid,'CREATE'::text)),
    EXISTS(SELECT 1 FROM protected_roles p WHERE pg_catalog.pg_has_role(me.oid,p.oid,'SET'::text) OR pg_catalog.pg_has_role(me.oid,p.oid,'USAGE'::text))
        OR EXISTS(SELECT 1 FROM pg_catalog.pg_auth_members m WHERE m.admin_option
            AND (pg_catalog.pg_has_role(me.oid,m.member,'SET'::text)
              OR pg_catalog.pg_has_role(me.oid,m.member,'USAGE'::text)))
        OR NOT EXISTS(SELECT 1 FROM pg_catalog.pg_roles dbowner WHERE dbowner.oid=db.datdba
            AND NOT dbowner.rolcanlogin AND NOT dbowner.rolsuper),
    pg_catalog.current_setting('transaction_read_only'::text)::boolean,
    pg_catalog.current_setting('transaction_isolation'::text)
FROM me CROSS JOIN db
"#;

fn schema_digest(material: &Value) -> Result<String, ServiceError> {
    let bytes = serde_json::to_vec(material).map_err(caused_by)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// The caller opens HTTP only after this same read-only transaction verifies
/// admission, the complete authenticated catalog material and formal histories.
pub(crate) async fn verify(
    pool: &PgPool,
    receipt: &Receipt,
    username: &str,
) -> Result<(), ServiceError> {
    let mut transaction = pool.begin().await.map_err(caused_by)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *transaction)
        .await
        .map_err(caused_by)?;
    sqlx::raw_sql(PROOF_SESSION_SETTINGS)
        .execute(&mut *transaction)
        .await
        .map_err(caused_by)?;
    let (
        database,
        comment,
        current_user,
        session_user,
        login,
        privileged,
        database_create,
        database_temporary,
        schema_create,
        owner_access,
        read_only,
        isolation,
    ) = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            String,
            String,
            bool,
            bool,
            bool,
            bool,
            bool,
            bool,
            bool,
            String,
        ),
    >(ADMISSION)
    .fetch_one(&mut *transaction)
    .await
    .map_err(caused_by)?;
    let proof = Proof {
        database,
        comment,
        current_user,
        session_user,
        login,
        privileged,
        database_create,
        database_temporary,
        schema_create,
        owner_access,
        read_only,
        isolation,
    };
    validate_proof(&proof, receipt, username)?;
    let material: Value = sqlx::query_scalar(include_str!("imported_schema.sql"))
        .fetch_one(&mut *transaction)
        .await
        .map_err(caused_by)?;
    if schema_digest(&material)? != receipt.schema_sha256 {
        return Err(failure());
    }
    // Readiness by itself is intentionally insufficient: full catalog SHA above
    // binds every structural object to the authenticated import evidence first.
    migrations::verify_connection(transaction.as_mut())
        .await
        .map_err(caused_by)?;
    transaction.commit().await.map_err(caused_by)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BATCH: &str = "019a1163-0000-7000-8000-000000000001";

    fn marker() -> String {
        format!("{MARKER_PREFIX}{BATCH}/{}", "a".repeat(64))
    }

    fn receipt() -> Receipt {
        Receipt {
            database: "imported_orders".into(),
            marker: marker(),
            schema_sha256: "b".repeat(64),
        }
    }

    fn mode(values: &[(&str, String)]) -> Result<Initialization, ConfigError> {
        Initialization::from_lookup(&|key| {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.clone())
        })
    }

    #[test]
    fn modes_require_a_complete_exclusive_canonical_receipt() {
        assert!(matches!(mode(&[]), Ok(Initialization::Seed)));
        assert!(matches!(
            mode(&[("WORK_ORDERS_DATABASE_INIT_MODE", "seed".into())]),
            Ok(Initialization::Seed)
        ));
        assert!(matches!(
            mode(&[("WORK_ORDERS_DATABASE_INIT_MODE", "import".into())]),
            Err(ConfigError::ImportRequiresMigrationCommand)
        ));
        let fields = [
            ("WORK_ORDERS_DATABASE_INIT_MODE", "imported".into()),
            ("WORK_ORDERS_EXPECTED_DATABASE", "imported_orders".into()),
            ("WORK_ORDERS_IMPORT_MARKER", marker()),
            ("WORK_ORDERS_EXPECTED_SCHEMA_SHA256", "b".repeat(64)),
        ];
        assert!(matches!(mode(&fields), Ok(Initialization::Imported(_))));
        for absent in 1..fields.len() {
            let values = fields
                .iter()
                .enumerate()
                .filter(|(n, _)| *n != absent)
                .map(|(_, pair)| pair.clone())
                .collect::<Vec<_>>();
            assert!(matches!(mode(&values), Err(ConfigError::ImportReceipt)));
        }
        for field in &fields[1..] {
            assert!(matches!(
                mode(std::slice::from_ref(field)),
                Err(ConfigError::ImportReceipt)
            ));
            assert!(matches!(
                mode(&[
                    ("WORK_ORDERS_DATABASE_INIT_MODE", "seed".into()),
                    field.clone(),
                ]),
                Err(ConfigError::ImportReceipt)
            ));
        }
        for bad in ["", "SEED", " imported", "seed,imported"] {
            assert!(matches!(
                mode(&[("WORK_ORDERS_DATABASE_INIT_MODE", bad.into())]),
                Err(ConfigError::DatabaseInitMode)
            ));
        }
        for bad in [
            marker().replace(BATCH, "019a1163-0000-4000-8000-000000000001"),
            marker().replace(BATCH, "019a1163-0000-7000-c000-000000000001"),
            marker().replace(BATCH, &BATCH.to_uppercase()),
            format!("{}extra", marker()),
            marker().replace(&"a".repeat(64), &"A".repeat(64)),
        ] {
            assert!(!valid_marker(&bad));
        }
        assert!(!valid_digest(&"A".repeat(64)));
        assert!(!valid_database("private\0canary"));
    }

    #[test]
    fn configured_target_is_the_decoded_database_and_rejects_control_injection() {
        let settings = Initialization::Imported(receipt());
        for raw in [
            "postgresql://runtime@127.0.0.1/imported_orders",
            "postgresql://runtime@127.0.0.1/%69mported_orders",
        ] {
            let database: PgConnectOptions = raw.parse().unwrap();
            assert!(settings.validate_target(&database).is_ok());
        }
        for raw in [
            "postgresql://runtime@127.0.0.1/other",
            "postgresql://runtime@127.0.0.1/imported_orders%00options%00-c%20role%3Dpostgres",
            "postgresql://runtime%00options%00-c%20role%3Dpostgres@127.0.0.1/imported_orders",
        ] {
            let database: PgConnectOptions = raw.parse().unwrap();
            assert!(matches!(
                settings.validate_target(&database),
                Err(ConfigError::ImportTarget)
            ));
        }
    }

    fn proof() -> Value {
        json!({"database":"imported_orders","comment":marker(),"current_user":"runtime","session_user":"runtime",
            "login":true,"privileged":false,"database_create":false,"database_temporary":false,"schema_create":false,"owner_access":false,
            "read_only":true,"isolation":"repeatable read"})
    }

    #[test]
    fn proof_rejects_wrong_target_receipt_identity_privileges_and_transaction_mode() {
        let valid: Proof = serde_json::from_value(proof()).unwrap();
        validate_proof(&valid, &receipt(), "runtime").unwrap();
        for (key, value) in [
            ("database", json!("other")),
            ("comment", Value::Null),
            ("comment", json!("private-canary")),
            ("current_user", json!("owner")),
            ("session_user", json!("postgres")),
            ("login", json!(false)),
            ("privileged", json!(true)),
            ("database_create", json!(true)),
            ("database_temporary", json!(true)),
            ("schema_create", json!(true)),
            ("owner_access", json!(true)),
            ("read_only", json!(false)),
            ("isolation", json!("read committed")),
        ] {
            let mut changed = proof();
            changed[key] = value;
            let parsed: Proof = serde_json::from_value(changed).unwrap();
            let error = validate_proof(&parsed, &receipt(), "runtime").unwrap_err();
            assert!(!format!("{error} {error:?}").contains("private-canary"));
        }
    }

    #[test]
    fn complete_schema_material_has_a_canonical_known_hash_and_no_lossy_filtering() {
        assert_eq!(
            PROOF_SESSION_SETTINGS
                .split(';')
                .map(str::trim)
                .filter(|statement| !statement.is_empty())
                .collect::<Vec<_>>(),
            [
                "SET LOCAL search_path = pg_catalog",
                "SET LOCAL timezone = 'UTC'",
                "SET LOCAL row_security = off",
            ]
        );
        assert_eq!(
            schema_digest(&json!([])).unwrap(),
            "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
        );
        let original = json!([{"kind":"table","name":"orders","constraint":"enabled"}]);
        let mut changed = original.clone();
        changed[0]["constraint"] = json!("disabled");
        assert_ne!(
            schema_digest(&original).unwrap(),
            schema_digest(&changed).unwrap()
        );
        assert!(ADMISSION.contains("'SET'"));
        assert!(ADMISSION.contains("'USAGE'"));
        // A runtime role must not be able to re-grant a membership with SET,
        // even when that membership currently disables SET and inheritance.
        assert!(ADMISSION.contains("FROM pg_catalog.pg_auth_members m WHERE m.admin_option"));
        assert!(ADMISSION.contains("pg_catalog.pg_has_role(me.oid,m.member,'SET'::text)"));
        assert!(ADMISSION.contains("pg_catalog.pg_has_role(me.oid,m.member,'USAGE'::text)"));
        assert!(ADMISSION.contains("FROM pg_catalog.pg_roles dbowner WHERE dbowner.oid=db.datdba"));
        assert!(ADMISSION.contains("AND NOT dbowner.rolcanlogin AND NOT dbowner.rolsuper"));
    }
}
