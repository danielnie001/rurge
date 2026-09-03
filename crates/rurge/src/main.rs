mod capabilities;
mod cli;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "rurge",
    version,
    about = "Cross-platform Surge-compatible network proxy"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print version information
    Version,
    /// Validate a Surge-format profile and print diagnostics
    Check(cli::check::CheckArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Version => {
            println!(
                "rurge {} (config {})",
                env!("CARGO_PKG_VERSION"),
                rurge_config::CRATE_VERSION
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Check(args) => cli::check::run(args),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
