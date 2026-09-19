use crate::capabilities;
use clap::Args;
use rurge_config::config::{LoadOptions, Platform};
use rurge_config::diagnostic::Severity;
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Args)]
pub struct CheckArgs {
    /// Profile to validate
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Print diagnostics as JSON
    #[arg(long)]
    pub json: bool,
    /// Exit with code 1 when there are warnings
    #[arg(long)]
    pub strict: bool,
    /// Evaluate the profile as if running on this platform (windows, linux, macos)
    #[arg(long, value_parser = parse_platform)]
    pub platform: Option<Platform>,
    /// Override the CORE_VERSION reported to requirement expressions
    #[arg(long)]
    pub core_version: Option<u64>,
}

pub(crate) fn parse_platform(s: &str) -> Result<Platform, String> {
    Platform::parse(s)
        .ok_or_else(|| format!("unknown platform `{s}` (expected windows, linux or macos)"))
}

#[derive(Serialize)]
struct Report<'a> {
    file: String,
    ok: bool,
    errors: usize,
    warnings: usize,
    infos: usize,
    diagnostics: Vec<&'a rurge_config::Diagnostic>,
}

pub fn run(args: CheckArgs) -> anyhow::Result<ExitCode> {
    let platform = args.platform.unwrap_or_else(Platform::current);
    let opts = LoadOptions {
        environment: super::environment(
            platform,
            args.core_version.unwrap_or(capabilities::CORE_VERSION),
        ),
        platform,
        capabilities: capabilities::current(),
    };
    let loaded = rurge_engine::load_checked(&args.config, &opts)?;
    let diags = loaded.diagnostics.sorted();
    let count = |s: Severity| diags.iter().filter(|d| d.severity == s).count();
    let (errors, warnings, infos) = (
        count(Severity::Error),
        count(Severity::Warning),
        count(Severity::Info),
    );

    if args.json {
        let report = Report {
            file: args.config.display().to_string(),
            ok: errors == 0,
            errors,
            warnings,
            infos,
            diagnostics: diags.iter().collect(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for d in diags.iter() {
            println!("{d}");
        }
        println!(
            "{}: {errors} error(s), {warnings} warning(s), {infos} note(s)",
            args.config.display()
        );
    }

    Ok(if errors > 0 {
        ExitCode::from(2)
    } else if warnings > 0 && args.strict {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}
