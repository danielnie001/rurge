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
use rurge_engine::{Engine, ListenerSpec, Running, Runtime, RuntimeOptions};
use rurge_rules::OutboundMode;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// How long a graceful shutdown waits for active sessions before force-cancelling them.
const GRACE: Duration = Duration::from_secs(5);
/// A burst of `--watch` file events inside this window collapses into one reload.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
/// How long a reload waits for an old listener's socket to close before rebinding.
const REBIND_WAIT: Duration = Duration::from_secs(2);

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
    /// Also write logs to this file, rotated daily (7 kept)
    #[arg(long, env = "RURGE_LOG_FILE", value_name = "PATH")]
    pub log_file: Option<PathBuf>,
    /// Evaluate the profile as if running on this platform
    #[arg(long, value_parser = super::check::parse_platform)]
    pub platform: Option<Platform>,
    /// Close a session after this many seconds with no traffic either way (default 600)
    #[arg(long, env = "RURGE_IDLE_TIMEOUT", value_name = "SECS")]
    pub idle_timeout: Option<u64>,
    /// Keep this many finished requests in the in-memory log (default 1000)
    #[arg(long, env = "RURGE_REQUEST_LOG_SIZE", value_name = "N")]
    pub request_log_size: Option<usize>,
    /// Reload the profile when it or its included files change on disk
    #[arg(long, env = "RURGE_WATCH")]
    pub watch: bool,
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

fn init_logging(
    level: LevelFilter,
    log_file: Option<&std::path::Path>,
) -> anyhow::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal());
    let registry = tracing_subscriber::registry()
        .with(level)
        .with(stdout_layer);
    match log_file {
        Some(path) => {
            let dir = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            let prefix = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "rurge.log".to_string());
            let appender = tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix(prefix)
                .max_log_files(7)
                .build(dir)
                .with_context(|| format!("cannot open log file {}", path.display()))?;
            let (nb, guard) = tracing_appender::non_blocking(appender);
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(nb);
            let _ = registry.with(file_layer).try_init();
            Ok(Some(guard))
        }
        None => {
            let _ = registry.try_init();
            Ok(None)
        }
    }
}

fn mode_name(mode: &OutboundMode) -> String {
    match mode {
        OutboundMode::Direct => "direct".to_string(),
        OutboundMode::Proxy(p) => format!("proxy={p}"),
        OutboundMode::Rule => "rule".to_string(),
    }
}

/// The run-only knobs (from `RunArgs`) a reload has to carry over unchanged.
struct RunOptions {
    idle_timeout: Duration,
    request_log_size: usize,
}

/// Builds one config generation. Used both at startup and on every reload.
async fn build_engine_runtime(
    cfg: rurge_config::Config,
    rt: &super::runtime::Runtime,
    run_opts: &RunOptions,
    outbound_mode: OutboundMode,
) -> anyhow::Result<Runtime> {
    let state = State::load(&rt.data_dir.join(STATE_FILE));
    let selections = state.selections_for(&profile_key(&cfg.source.main));
    Runtime::build(
        cfg,
        RuntimeOptions {
            stack: rt.stack_options(Duration::ZERO),
            outbound_mode,
            selections,
            idle_timeout: run_opts.idle_timeout,
            request_log_size: run_opts.request_log_size,
        },
    )
    .await
    .context("cannot build the runtime")
}

fn print_listening(listeners: &[(ListenerSpec, Running)]) {
    for (spec, running) in listeners {
        let scheme = match spec.kind {
            ListenerKind::Socks5 => "socks5",
            _ => "http",
        };
        println!("listening on {scheme}://{}", running.local_addr);
    }
}

/// Reloads the profile from disk and swaps it in (M3b design §7.4). Every
/// failure path keeps the running config, so a bad edit never takes the
/// daemon down.
async fn reload(
    engine: &Arc<Engine>,
    config: &Path,
    load_opts: &LoadOptions,
    rt: &super::runtime::Runtime,
    run_opts: &RunOptions,
    outbound_mode: &OutboundMode,
    listeners: &mut Vec<(ListenerSpec, Running)>,
) {
    let loaded = match load(config, load_opts) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: reload failed, keeping current config: {e}");
            return;
        }
    };
    if loaded.diagnostics.has_errors() {
        eprintln!("reload failed, keeping current config:");
        print_diagnostics(&loaded.diagnostics.sorted());
        return;
    }
    print_diagnostics(&loaded.diagnostics.sorted());
    let next = match build_engine_runtime(loaded.config, rt, run_opts, outbound_mode.clone()).await
    {
        Ok(n) => n,
        Err(e) => {
            eprintln!("error: reload failed, keeping current config: {e}");
            return;
        }
    };
    print_diagnostics(next.diagnostics());
    if engine.swap_runtime(next) {
        // The listen addresses changed. Stop the old accept loops, wait until
        // their sockets are closed (so the addresses can be bound again;
        // Windows sets no `SO_REUSEADDR`), then let their in-flight sessions
        // drain in the background — dropping the old `Running`s would abort them.
        let olds: Vec<Running> = listeners.drain(..).map(|(_, r)| r).collect();
        for old in &olds {
            old.stop();
        }
        for old in &olds {
            let _ = tokio::time::timeout(REBIND_WAIT, old.wait_closed()).await;
        }
        for old in olds {
            tokio::spawn(old.join());
        }
        match engine.bind_listeners().await {
            Ok(next_listeners) => {
                *listeners = next_listeners;
                print_listening(listeners);
            }
            Err(e) => {
                eprintln!("error: reload could not rebind listeners: {e}");
                return;
            }
        }
    }
    tracing::info!("profile reloaded");
}

/// Watches the profile and its includes, reporting each debounced burst of
/// changes as one `()` on `tx`. The returned watcher must be kept alive:
/// dropping it stops the watch.
fn spawn_watcher(
    paths: &[PathBuf],
    tx: tokio::sync::mpsc::Sender<()>,
) -> anyhow::Result<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let (raw_tx, raw_rx) = std::sync::mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = raw_tx.send(());
        }
    })?;
    for p in paths {
        // Watch the parent directory: editors replace files rather than modify
        // them. Canonicalize first, a relative `-c t.conf` has an empty parent.
        let full = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let dir = full
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
            eprintln!("warning: cannot watch {}: {e}", dir.display());
        }
    }
    std::thread::spawn(move || {
        while raw_rx.recv().is_ok() {
            // collapse the rest of the burst into this one reload
            while raw_rx.recv_timeout(WATCH_DEBOUNCE).is_ok() {}
            if tx.blocking_send(()).is_err() {
                break;
            }
        }
    });
    Ok(watcher)
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
    let _log_guard = init_logging(
        args.log_level
            .unwrap_or_else(|| level_for(&cfg.general.loglevel)),
        args.log_file.as_deref(),
    )?;
    let rt = args.runtime.resolve(&cfg)?;
    let outbound_mode = args.outbound_mode.clone();
    let run_opts = RunOptions {
        idle_timeout: Duration::from_secs(args.idle_timeout.unwrap_or(600).max(1)),
        request_log_size: args.request_log_size.unwrap_or(1000).max(1),
    };
    // `cfg` is moved into the runtime below, so collect the watch list first.
    let cfg_paths: Vec<PathBuf> = std::iter::once(cfg.source.main.clone())
        .chain(cfg.source.includes.iter().cloned())
        .collect();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let engine_rt = build_engine_runtime(cfg, &rt, &run_opts, outbound_mode.clone()).await?;
        print_diagnostics(engine_rt.diagnostics());
        let (policies, rules) = (
            engine_rt.policies.names().len(),
            engine_rt.rules.rules().len(),
        );
        let engine = Engine::new(engine_rt);
        engine.start_sampler();
        let mut listeners = match engine.bind_listeners().await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: cannot bind listener: {e}");
                return Ok(ExitCode::from(1));
            }
        };
        print_listening(&listeners);
        println!(
            "rurge {} running: {policies} policies, {rules} rules, outbound mode {}",
            env!("CARGO_PKG_VERSION"),
            mode_name(&outbound_mode)
        );

        // Reload triggers. `reload_tx` stays alive here on purpose: were every
        // sender dropped, `recv()` would return `None` at once and the loop
        // below would spin reloading.
        let (reload_tx, mut reload_rx) = tokio::sync::mpsc::channel::<()>(1);
        let _watcher = if args.watch {
            Some(spawn_watcher(&cfg_paths, reload_tx.clone())?)
        } else {
            None
        };
        // One long-lived stream per signal, built once outside the loop: a
        // stream buffers a signal that arrives while nobody is awaiting it,
        // whereas a fresh `signal::ctrl_c()` future per iteration would drop
        // every Ctrl-C delivered during a `reload`.
        #[cfg(unix)]
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .context("cannot listen for SIGINT")?;
        #[cfg(windows)]
        let mut interrupt = tokio::signal::windows::ctrl_c().context("cannot listen for Ctrl-C")?;
        #[cfg(unix)]
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .context("cannot listen for SIGHUP")?;
        #[cfg(unix)]
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("cannot listen for SIGTERM")?;

        loop {
            // Ctrl-C everywhere, SIGTERM as well on Unix (M3b design §7.4).
            let shutdown_signal = async {
                #[cfg(unix)]
                {
                    tokio::select! {
                        _ = interrupt.recv() => {}
                        _ = sigterm.recv() => {}
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = interrupt.recv().await;
                }
            };
            let reload_signal = async {
                #[cfg(unix)]
                {
                    tokio::select! {
                        _ = sighup.recv() => {}
                        _ = reload_rx.recv() => {}
                    }
                }
                #[cfg(not(unix))]
                {
                    reload_rx.recv().await;
                }
            };
            tokio::select! {
                _ = shutdown_signal => break,
                _ = reload_signal => {
                    reload(
                        &engine,
                        &args.config,
                        &opts,
                        &rt,
                        &run_opts,
                        &outbound_mode,
                        &mut listeners,
                    )
                    .await;
                }
            }
        }

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
        tokio::pin!(drain);
        tokio::select! {
            _ = &mut drain => {}
            _ = tokio::signal::ctrl_c() => {
                println!("forced shutdown");
                return Ok(ExitCode::SUCCESS);
            }
            _ = tokio::time::sleep(GRACE) => {
                println!("grace period elapsed; closing active sessions");
                engine.cancel_sessions();
                // relays race the session token, so this completes quickly;
                // bound it anyway and exit regardless
                let _ = tokio::time::timeout(Duration::from_secs(1), &mut drain).await;
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
