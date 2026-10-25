//! All process-level cases fail during configuration, before any database opens.
use std::process::Command;

use work_orders_api::{Config, ConfigError};

const MARKER: &str = "saas-uuid-import/1/019a1163-0000-7000-8000-000000000001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn settings() -> Vec<(&'static str, String)> {
    vec![
        (
            "WORK_ORDERS_DATABASE_URL",
            "postgresql://runtime:private-canary@127.0.0.1:1/%69mported_orders".into(),
        ),
        (
            "WORK_ORDERS_ALLOWED_ORIGINS",
            "http://127.0.0.1:3000".into(),
        ),
        ("WORK_ORDERS_DATABASE_INIT_MODE", "imported".into()),
        ("WORK_ORDERS_EXPECTED_DATABASE", "imported_orders".into()),
        ("WORK_ORDERS_IMPORT_MARKER", MARKER.into()),
        ("WORK_ORDERS_EXPECTED_SCHEMA_SHA256", HASH.into()),
    ]
}

#[test]
fn public_config_requires_the_complete_receipt_and_exact_decoded_database() {
    Config::from_pairs(settings()).unwrap();
    for field in [
        "WORK_ORDERS_EXPECTED_DATABASE",
        "WORK_ORDERS_IMPORT_MARKER",
        "WORK_ORDERS_EXPECTED_SCHEMA_SHA256",
    ] {
        let mut values = settings();
        values.retain(|(name, _)| *name != field);
        assert_eq!(
            Config::from_pairs(values).unwrap_err(),
            ConfigError::ImportReceipt
        );
    }
    for (field, value, expected) in [
        (
            "WORK_ORDERS_DATABASE_INIT_MODE",
            "seed",
            ConfigError::ImportReceipt,
        ),
        (
            "WORK_ORDERS_DATABASE_INIT_MODE",
            "import",
            ConfigError::ImportRequiresMigrationCommand,
        ),
        (
            "WORK_ORDERS_DATABASE_INIT_MODE",
            "seed,imported",
            ConfigError::DatabaseInitMode,
        ),
        (
            "WORK_ORDERS_EXPECTED_DATABASE",
            "private-canary",
            ConfigError::ImportTarget,
        ),
        (
            "WORK_ORDERS_IMPORT_MARKER",
            "private-canary",
            ConfigError::ImportReceipt,
        ),
        (
            "WORK_ORDERS_EXPECTED_SCHEMA_SHA256",
            "private-canary",
            ConfigError::ImportReceipt,
        ),
    ] {
        let mut values = settings();
        values
            .iter_mut()
            .find(|(name, _)| *name == field)
            .unwrap()
            .1 = value.into();
        let error = Config::from_pairs(values).unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error} {error:?}").contains("private-canary"));
    }
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_work-orders-api"));
    command.env_clear();
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    command
}

#[test]
fn normal_binary_rejects_import_or_orphan_receipts_before_runtime_configuration() {
    for (name, value, expected) in [
        (
            "WORK_ORDERS_DATABASE_INIT_MODE",
            "import",
            "IMPORT_REQUIRES_MIGRATE_ONLY",
        ),
        (
            "WORK_ORDERS_DATABASE_INIT_MODE",
            "imported",
            "IMPORTED_DATABASE_RECEIPT_INVALID",
        ),
        (
            "WORK_ORDERS_DATABASE_INIT_MODE",
            "private-canary",
            "WORK_ORDERS_DATABASE_INIT_MODE must be",
        ),
        (
            "WORK_ORDERS_EXPECTED_DATABASE",
            "private-canary",
            "IMPORTED_DATABASE_RECEIPT_INVALID",
        ),
        (
            "WORK_ORDERS_IMPORT_MARKER",
            "private-canary",
            "IMPORTED_DATABASE_RECEIPT_INVALID",
        ),
        (
            "WORK_ORDERS_EXPECTED_SCHEMA_SHA256",
            "private-canary",
            "IMPORTED_DATABASE_RECEIPT_INVALID",
        ),
    ] {
        let output = command().env(name, value).output().unwrap();
        assert!(!output.status.success());
        let text = format!(
            "{}{}",
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap()
        );
        assert!(text.contains(expected), "{text}");
        assert!(!text.contains("private-canary"));
    }
}

#[cfg(unix)]
#[test]
fn non_unicode_mode_cannot_become_default_seed() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let output = command()
        .env(
            "WORK_ORDERS_DATABASE_INIT_MODE",
            OsString::from_vec(vec![0xff]),
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap()
    );
    assert!(text.contains("IMPORTED_DATABASE_RECEIPT_INVALID"));
}
