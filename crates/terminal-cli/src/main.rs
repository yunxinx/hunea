use color_eyre::eyre::Result;
use std::{ffi::OsStr, process::ExitCode};

fn main() -> Result<ExitCode> {
    if is_version_request() {
        println!("hunea {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }

    color_eyre::install()?;
    terminal_app::install_terminal_panic_hook();
    match terminal_app::run_for_cli() {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(terminal_app::AppRunError::Config(error)) => {
            eprintln!("{}", app_config::startup_error::format_config_error(&error));
            Ok(ExitCode::FAILURE)
        }
        Err(terminal_app::AppRunError::Runtime(error)) => Err(error),
    }
}

fn is_version_request() -> bool {
    let mut args = std::env::args_os().skip(1);
    matches!(args.next().as_deref(), Some(flag) if flag == OsStr::new("--version") || flag == OsStr::new("-V"))
        && args.next().is_none()
}
