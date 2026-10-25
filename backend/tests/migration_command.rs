//! Subprocess checks exercise argument/configuration rejection before any
//! database can be contacted. These tests never require a PostgreSQL server.

use std::{
    ffi::OsStr,
    process::{Command, Output},
};

const URL: &str = "postgresql://private-migration-user:private-migration-password@127.0.0.1:1/private-migration-db";

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_work-orders-migrate"));
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("PG")
        {
            command.env_remove(name);
        }
    }
    command
        .env_remove("WORK_ORDERS_DATABASE_URL")
        .env_remove("WORK_ORDERS_ALLOWED_ORIGINS")
        .env_remove("PLATFORM_API_ORIGIN")
        .env_remove("PLATFORM_CLIENT_ID")
        .env_remove("PLATFORM_CLIENT_SECRET");
    command
}

fn failure(output: Output, expected: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(expected), "{stderr}");
    for private in [
        "private-migration-",
        "private-argument",
        "private-driver-value",
    ] {
        assert!(!stderr.contains(private), "{stderr}");
    }
}

#[test]
fn unsupported_conflicting_unknown_and_repeated_arguments_fail_before_url_validation() {
    for (arguments, expected) in [
        (vec!["--seed"], "--seed is not supported"),
        (
            vec!["--seed", "--import"],
            "--seed and --import are mutually exclusive",
        ),
        (
            vec!["--import", "--seed"],
            "--seed and --import are mutually exclusive",
        ),
        (vec!["--import", "--import"], "invalid migration arguments"),
        (vec!["--seed", "--seed"], "invalid migration arguments"),
        (vec!["private-argument"], "invalid migration arguments"),
    ] {
        failure(
            command()
                .args(arguments)
                .env("WORK_ORDERS_DATABASE_URL", "private-invalid-url")
                .output()
                .unwrap(),
            expected,
        );
    }
}

#[test]
fn default_and_import_require_only_the_dedicated_url_without_fallback() {
    for arguments in [vec![], vec!["--import"]] {
        failure(
            command()
                .args(arguments)
                .env("DATABASE_URL", URL)
                .env("STORES_DATABASE_URL", URL)
                .output()
                .unwrap(),
            "WORK_ORDERS_DATABASE_URL must explicitly identify",
        );
    }
    failure(
        command()
            .env("WORK_ORDERS_DATABASE_URL", "private-invalid-url")
            .output()
            .unwrap(),
        "WORK_ORDERS_DATABASE_URL must explicitly identify",
    );
}

#[test]
fn pg_environment_options_are_rejected_before_connecting() {
    for key in ["PGPORT", "PGPASSWORD", "PGOPTIONS"] {
        failure(
            command()
                .arg("--import")
                .env("WORK_ORDERS_DATABASE_URL", URL)
                .env(key, "private-driver-value")
                .output()
                .unwrap(),
            "PG-prefixed environment variables are not supported",
        );
    }
}

#[test]
fn environment_files_in_the_working_directory_are_not_loaded() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "work-orders-migrate-no-dotenv-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0.join(".env"));
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    std::fs::write(
        directory.join(".env"),
        format!("WORK_ORDERS_DATABASE_URL={URL}\n"),
    )
    .unwrap();
    failure(
        command()
            .current_dir(&directory)
            .arg(OsStr::new("--import"))
            .output()
            .unwrap(),
        "WORK_ORDERS_DATABASE_URL must explicitly identify",
    );
}
