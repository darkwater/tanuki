use std::process::ExitCode;
use tanuki_ui::{
    cli::{Cli, HELP},
    format::safe_text,
    terminal,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let config = match Cli::parse(std::env::args_os().skip(1)) {
        Ok(Cli::Help) => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Ok(Cli::Run(config)) => config,
        Err(error) => {
            eprintln!("{}\n{HELP}", safe_text(&error.to_string()));
            return ExitCode::FAILURE;
        }
    };
    match terminal::run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", safe_text(&error.to_string()));
            ExitCode::FAILURE
        }
    }
}
