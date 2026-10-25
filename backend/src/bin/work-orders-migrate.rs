use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match work_orders_api::run_migration_command(std::env::args_os().skip(1)).await {
        Ok(()) => {
            println!("work-orders database migrations verified");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("work-orders migration failed: {error}");
            ExitCode::FAILURE
        }
    }
}
