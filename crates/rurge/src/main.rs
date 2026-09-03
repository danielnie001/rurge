use clap::{Parser, Subcommand};

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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => {
            println!(
                "rurge {} (config {})",
                env!("CARGO_PKG_VERSION"),
                rurge_config::CRATE_VERSION
            );
        }
    }
    Ok(())
}
