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
    /// Rule engine tools (offline)
    Rule(Box<cli::rule::RuleArgs>),
    /// DNS tools (offline)
    Dns(Box<cli::dns::DnsArgs>),
    /// Run the proxy in the foreground
    Run(Box<cli::run::RunArgs>),
    /// Reload the running daemon's profile (needs http-api)
    Reload(cli::control::ControlArgs),
    /// Stop the running daemon (needs http-api)
    Stop(cli::control::ControlArgs),
    /// Show the running daemon's mode, counts and traffic (needs http-api)
    Status(cli::control::StatusArgs),
    /// Install or remove the automatic start of rurge
    Service(cli::service::ServiceArgs),
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
        Command::Rule(args) => cli::rule::run(*args),
        Command::Dns(args) => cli::dns::run(*args),
        Command::Run(args) => cli::run::run(*args),
        Command::Reload(args) => cli::control::reload(args),
        Command::Stop(args) => cli::control::stop(args),
        Command::Status(args) => cli::control::status(args),
        Command::Service(args) => cli::service::run(args),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
