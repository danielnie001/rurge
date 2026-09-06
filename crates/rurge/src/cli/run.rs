//! `rurge run`: the foreground proxy daemon (M3 design §9.3).

use super::rule::{parse_mode, print_diagnostics};
use super::runtime::RuntimeArgs;
use crate::capabilities;
use anyhow::Context;
use clap::Args;
use rurge_config::config::{LoadOptions, Platform, load};
use rurge_config::general::LogLevel;
use rurge_config::session::ListenerKind;
use rurge_engine::state::{STATE_FILE, State, profile_key};
use rurge_engine::{Engine, Runtime, RuntimeOptions};
use rurge_rules::OutboundMode;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tracing_subscriber::filter::LevelFilter;

/// How long a graceful shutdown waits for active sessions before force-cancelling them.
const GRACE: Duration = Duration::from_secs(5);

#[derive(Args)]
pub struct RunArgs {
    /// Profile to load
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Outbound mode: direct, proxy=<policy>, rule
    #[arg(long, env = "RURGE_OUTBOUND_MODE", value_parser = parse_mode, default_value = "rule")]
    pub outbound_mode: OutboundMode,
    /// Log level override: verbose|info|notify|warning (also debug|error)
    #[arg(long, env = "RURGE_LOG_LEVEL", value_parser = parse_log_level, value_name = "LEVEL")]
    pub log_level: Option<LevelFilter>,
    /// Evaluate the profile as if running on this platform
    #[arg(long, value_parser = super::check::parse_platform)]
    pub platform: Option<Platform>,
    /// Close a session after this many seconds with no traffic either way (default 600)
    #[arg(long, env = "RURGE_IDLE_TIMEOUT", value_name = "SECS")]
    pub idle_timeout: Option<u64>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
}

pub(crate) fn parse_log_level(s: &str) -> Result<LevelFilter, String> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "verbose" | "trace" => LevelFilter::TRACE,
        "info" | "debug" => LevelFilter::DEBUG,
        "notify" => LevelFilter::INFO,
        "warning" | "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        other => {
            return Err(format!(
                "unknown log level `{other}` (expected verbose, info, notify, warning, debug or error)"
            ));
        }
    })
}

/// Surge `loglevel` → tracing level (phase 1 design §12).
pub(crate) fn level_for(level: &LogLevel) -> LevelFilter {
    match level {
        LogLevel::Verbose => LevelFilter::TRACE,
        LogLevel::Info => LevelFilter::DEBUG,
        LogLevel::Notify => LevelFilter::INFO,
        LogLevel::Warning => LevelFilter::WARN,
    }
}

fn init_logging(level: LevelFilter) {
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal())
        .try_init();
}

fn mode_name(mode: &OutboundMode) -> String {
    match mode {
        OutboundMode::Direct => "direct".to_string(),
        OutboundMode::Proxy(p) => format!("proxy={p}"),
        OutboundMode::Rule => "rule".to_string(),
    }
}

pub fn run(args: RunArgs) -> anyhow::Result<ExitCode> {
    let platform = args.platform.unwrap_or_else(Platform::current);
    let opts = LoadOptions {
        environment: super::environment(platform, capabilities::CORE_VERSION),
        platform,
        capabilities: capabilities::current(),
    };
    let loaded = load(&args.config, &opts)?;
    if loaded.diagnostics.has_errors() {
        print_diagnostics(&loaded.diagnostics.sorted());
        return Ok(ExitCode::from(2));
    }
    print_diagnostics(&loaded.diagnostics.sorted());
    let cfg = loaded.config;
    init_logging(
        args.log_level
            .unwrap_or_else(|| level_for(&cfg.general.loglevel)),
    );
    let rt = args.runtime.resolve(&cfg)?;
    let state = State::load(&rt.data_dir.join(STATE_FILE));
    let selections = state.selections_for(&profile_key(&cfg.source.main));
    let outbound_mode = args.outbound_mode.clone();
    let idle_timeout = Duration::from_secs(args.idle_timeout.unwrap_or(600).max(1));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let engine_rt = Runtime::build(
            cfg,
            RuntimeOptions {
                stack: rt.stack_options(Duration::ZERO),
                outbound_mode: outbound_mode.clone(),
                idle_timeout,
                selections,
            },
        )
        .await
        .context("cannot build the runtime")?;
        print_diagnostics(engine_rt.diagnostics());
        let (policies, rules) = (
            engine_rt.policies.names().len(),
            engine_rt.rules.rules().len(),
        );
        let engine = Engine::new(engine_rt);
        let listeners = match engine.bind_listeners().await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: cannot bind listener: {e}");
                return Ok(ExitCode::from(1));
            }
        };
        for (spec, running) in &listeners {
            let scheme = match spec.kind {
                ListenerKind::Socks5 => "socks5",
                _ => "http",
            };
            println!("listening on {scheme}://{}", running.local_addr);
        }
        println!(
            "rurge {} running: {policies} policies, {rules} rules, outbound mode {}",
            env!("CARGO_PKG_VERSION"),
            mode_name(&outbound_mode)
        );
        tokio::signal::ctrl_c()
            .await
            .context("cannot listen for Ctrl-C")?;
        println!("shutting down (Ctrl-C again to exit now)");
        engine.stop_accepting();
        engine.tracker().close();
        let running: Vec<_> = listeners.into_iter().map(|(_, r)| r).collect();
        let drain = async {
            for r in running {
                r.join().await;
            }
            engine.tracker().wait().await;
        };
        tokio::select! {
            _ = drain => {}
            _ = tokio::signal::ctrl_c() => {
                println!("forced shutdown");
                return Ok(ExitCode::SUCCESS);
            }
            _ = tokio::time::sleep(GRACE) => {
                println!("grace period elapsed; closing active sessions");
                engine.cancel_sessions();
                // brief wait for the relays to unwind, then exit regardless
                let _ = tokio::time::timeout(Duration::from_secs(1), async {
                    engine.tracker().wait().await;
                }).await;
            }
        }
        Ok(ExitCode::SUCCESS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_levels_map_like_surge() {
        assert_eq!(level_for(&LogLevel::Verbose), LevelFilter::TRACE);
        assert_eq!(level_for(&LogLevel::Info), LevelFilter::DEBUG);
        assert_eq!(level_for(&LogLevel::Notify), LevelFilter::INFO);
        assert_eq!(level_for(&LogLevel::Warning), LevelFilter::WARN);
        assert_eq!(parse_log_level("DEBUG").unwrap(), LevelFilter::DEBUG);
        assert_eq!(parse_log_level("error").unwrap(), LevelFilter::ERROR);
        assert!(parse_log_level("loud").is_err());
        assert_eq!(mode_name(&OutboundMode::Rule), "rule");
    }
}
