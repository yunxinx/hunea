use color_eyre::eyre::Result;
use std::process::ExitCode;

fn main() -> Result<ExitCode> {
    color_eyre::install()?;
    match lumos::app::run_for_cli() {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(lumos::app::AppRunError::Config(error)) => {
            eprintln!("{}", lumos::startup_error::format_config_error(&error));
            Ok(ExitCode::FAILURE)
        }
        Err(lumos::app::AppRunError::Runtime(error)) => Err(error),
    }
}
