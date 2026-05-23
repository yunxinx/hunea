use color_eyre::eyre::Result;
use std::process::ExitCode;

fn main() -> Result<ExitCode> {
    color_eyre::install()?;
    match terminal_app::run_for_cli() {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(terminal_app::AppRunError::Config(error)) => {
            eprintln!("{}", app_config::startup_error::format_config_error(&error));
            Ok(ExitCode::FAILURE)
        }
        Err(terminal_app::AppRunError::Runtime(error)) => Err(error),
    }
}
