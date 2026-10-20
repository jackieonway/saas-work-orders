use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .json()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    let config = match work_orders_api::Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(event = "work_orders_configuration_failed", category = %error);
            return ExitCode::FAILURE;
        }
    };
    match work_orders_api::run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(event = "work_orders_service_failed", category = %error);
            ExitCode::FAILURE
        }
    }
}
