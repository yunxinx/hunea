use color_eyre::eyre::Result;
use std::process::ExitCode;

fn main() -> Result<ExitCode> {
    color_eyre::install()?;
    match mo_app::run_for_cli() {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(mo_app::AppRunError::Config(error)) => {
            eprintln!("{}", mo_config::startup_error::format_config_error(&error));
            Ok(ExitCode::FAILURE)
        }
        Err(mo_app::AppRunError::Runtime(error)) => Err(error),
    }
}
