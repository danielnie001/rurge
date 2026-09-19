//! `rurge run`: the foreground proxy daemon (M3 design §9.3).

use super::rule::{parse_mode, print_diagnostics};
use super::runtime::RuntimeArgs;
use super::sysproxy::{SystemProxyManager, describe, proxy_settings};
use crate::capabilities;
use anyhow::Context;
use clap::Args;
use rurge_api::ApiContext;
use rurge_config::config::{LoadOptions, Platform, load};
use rurge_config::general::ControllerAccess;
use rurge_config::general::LogLevel;
use rurge_config::rule::PolicyRef;
use rurge_config::session::ListenerKind;
use rurge_engine::control::{Control, LogLevel as ApiLogLevel, Mode, ReloadReport};
use rurge_engine::state::{STATE_FILE, State, StateStore, profile_key};
use rurge_engine::{Engine, EngineShared, ListenerSpec, Running, Runtime, RuntimeOptions};
use rurge_net::BoxFuture;
use rurge_platform::sysproxy::ProxySettings;
use rurge_rules::OutboundMode;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::Registry;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;

/// How long a graceful shutdown waits for active sessions before force-cancelling them.
const GRACE: Duration = Duration::from_secs(5);
/// A burst of `--watch` file events inside this window collapses into one reload.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
/// The per-data-directory instance lock. Never deleted: removing it would race
/// with another process opening it.
const LOCK_FILE: &str = "rurge.lock";

#[derive(Args)]
pub struct RunArgs {
    /// Profile to load
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Outbound mode: direct, proxy=<policy>, rule (default: the last mode
    /// saved in state.json, else rule)
    #[arg(long, env = "RURGE_OUTBOUND_MODE", value_parser = parse_mode)]
    pub outbound_mode: Option<OutboundMode>,
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
    /// Point the operating system's proxy settings at rurge while it runs
    #[arg(long, env = "RURGE_SYSTEM_PROXY")]
    pub system_proxy: bool,
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

type LevelHandle = reload::Handle<LevelFilter, Registry>;

fn init_logging(
    level: LevelFilter,
    log_file: Option<&std::path::Path>,
) -> anyhow::Result<(
    Option<tracing_appender::non_blocking::WorkerGuard>,
    LevelHandle,
)> {
    let (level_layer, handle): (reload::Layer<LevelFilter, Registry>, LevelHandle) =
        reload::Layer::new(level);
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal());
    let registry = tracing_subscriber::registry()
        .with(level_layer)
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
            Ok((Some(guard), handle))
        }
        None => {
            let _ = registry.try_init();
            Ok((None, handle))
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

/// What the API can ask the main loop to do (M4 design §6).
enum Command {
    Reload(oneshot::Sender<ReloadReport>),
    Stop,
    SystemProxy(bool, oneshot::Sender<Result<(), String>>),
}

struct LoopControl {
    tx: mpsc::Sender<Command>,
    log_level: LevelHandle,
    /// Mirrors `SystemProxyManager::enabled`.
    system_proxy: Arc<AtomicBool>,
}

fn failed_report() -> ReloadReport {
    ReloadReport {
        ok: false,
        errors: 1,
        warnings: 0,
        listeners_rebound: false,
    }
}

impl Control for LoopControl {
    fn reload(&self) -> BoxFuture<'_, ReloadReport> {
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            if self.tx.send(Command::Reload(reply)).await.is_err() {
                return failed_report();
            }
            rx.await.unwrap_or_else(|_| failed_report())
        })
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.tx.send(Command::Stop).await;
        })
    }

    fn set_log_level(&self, level: ApiLogLevel) -> Result<(), String> {
        // the same mapping as `parse_log_level`
        let filter = match level {
            ApiLogLevel::Verbose => LevelFilter::TRACE,
            ApiLogLevel::Debug | ApiLogLevel::Info => LevelFilter::DEBUG,
            ApiLogLevel::Notify => LevelFilter::INFO,
            ApiLogLevel::Warning => LevelFilter::WARN,
            ApiLogLevel::Error => LevelFilter::ERROR,
        };
        self.log_level
            .modify(|f| *f = filter)
            .map_err(|e| e.to_string())?;
        tracing::info!(level = level.as_str(), "log level changed");
        Ok(())
    }

    fn set_system_proxy(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            self.tx
                .send(Command::SystemProxy(enabled, reply))
                .await
                .map_err(|_| "rurge is shutting down".to_string())?;
            rx.await
                .unwrap_or_else(|_| Err("rurge is shutting down".to_string()))
        })
    }

    fn system_proxy_enabled(&self) -> bool {
        self.system_proxy.load(Ordering::SeqCst)
    }
}

/// Builds one config generation. Used both at startup and on every reload.
async fn build_engine_runtime(
    cfg: rurge_config::Config,
    rt: &super::runtime::Runtime,
    run_opts: &RunOptions,
    outbound_mode: OutboundMode,
    shared: &EngineShared,
) -> anyhow::Result<Runtime> {
    Runtime::build(
        cfg,
        RuntimeOptions {
            stack: rt.stack_options(Duration::ZERO),
            outbound_mode,
            shared: shared.clone(),
            idle_timeout: run_opts.idle_timeout,
            request_log_size: run_opts.request_log_size,
        },
    )
    .await
    .context("cannot build the runtime")
}

/// Explicit flag > state.json > rule. An explicit value is written back.
async fn initial_mode(
    explicit: Option<OutboundMode>,
    store: &StateStore,
    state: &State,
) -> OutboundMode {
    if let Some(mode) = explicit {
        let (m, global) = Mode::from_outbound(&mode);
        store
            .update(|s| {
                s.outbound_mode = Some(m.as_str().to_string());
                if global.is_some() {
                    s.global_policy = global;
                }
            })
            .await;
        return mode;
    }
    match state.outbound_mode.as_deref().and_then(Mode::parse) {
        Some(Mode::Direct) => OutboundMode::Direct,
        Some(Mode::Proxy) => match &state.global_policy {
            Some(p) => OutboundMode::Proxy(PolicyRef::parse(p)),
            None => {
                tracing::warn!(
                    "state.json says proxy mode but names no global policy; using rule mode"
                );
                OutboundMode::Rule
            }
        },
        Some(Mode::Rule) | None => OutboundMode::Rule,
    }
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

/// Everything a reload needs besides the listeners it swaps.
struct Daemon<'a> {
    engine: &'a Arc<Engine>,
    config: &'a Path,
    load_opts: &'a LoadOptions,
    rt: &'a super::runtime::Runtime,
    run_opts: &'a RunOptions,
    outbound_mode: &'a OutboundMode,
    /// Not read by this task's reload path (selections now come from
    /// `engine.shared()`); kept for the state features Task 8 adds here.
    #[allow(dead_code)]
    store: &'a StateStore,
    http_api: &'a Option<ControllerAccess>,
}

fn count(diags: &rurge_config::Diagnostics, severity: rurge_config::Severity) -> usize {
    diags.iter().filter(|d| d.severity == severity).count()
}

/// Reloads the profile from disk and swaps it in (M3b design §7.4). Every
/// failure path keeps the running config, so a bad edit never takes the
/// daemon down.
async fn reload(d: &Daemon<'_>, listeners: &mut Vec<(ListenerSpec, Running)>) -> ReloadReport {
    use rurge_config::Severity;
    let loaded = match load(d.config, d.load_opts) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: reload failed, keeping current config: {e}");
            return failed_report();
        }
    };
    let errors = count(&loaded.diagnostics, Severity::Error);
    let warnings = count(&loaded.diagnostics, Severity::Warning);
    if errors > 0 {
        eprintln!("reload failed, keeping current config:");
        print_diagnostics(&loaded.diagnostics.sorted());
        return ReloadReport {
            ok: false,
            errors,
            warnings,
            listeners_rebound: false,
        };
    }
    print_diagnostics(&loaded.diagnostics.sorted());
    if &loaded.config.general.http_api != d.http_api {
        // `d.http_api` is the profile as it was at boot, and the daemon exits
        // when a configured API cannot bind, so `is_some()` means an API is
        // actually running; otherwise there is nothing to keep.
        let message = if d.http_api.is_some() {
            "http-api changed in the profile; the API keeps its current address and key until rurge restarts"
        } else {
            "http-api was added to the profile; the API is not started until rurge restarts"
        };
        tracing::warn!("{message}");
    }
    let next = match build_engine_runtime(
        loaded.config,
        d.rt,
        d.run_opts,
        d.outbound_mode.clone(),
        &d.engine.shared(),
    )
    .await
    {
        Ok(n) => n,
        Err(e) => {
            eprintln!("error: reload failed, keeping current config: {e}");
            return ReloadReport {
                ok: false,
                errors: 1,
                warnings,
                listeners_rebound: false,
            };
        }
    };
    print_diagnostics(next.diagnostics());
    let surface_changed = d.engine.swap_runtime(next);
    let mut rebound = false;
    // An empty list is the degraded state a failed rebind leaves behind: try
    // again even when the listener surface is unchanged, so freeing the
    // conflicting port and reloading the same profile brings the daemon back.
    if surface_changed || listeners.is_empty() {
        match d.engine.rebind_listeners(std::mem::take(listeners)).await {
            Ok(next_listeners) => {
                *listeners = next_listeners;
                print_listening(listeners);
                rebound = true;
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "reload could not rebind listeners; the daemon has no listeners until the next successful reload"
                );
                eprintln!(
                    "error: reload could not rebind listeners: {e}; the daemon has no listeners until the next successful reload"
                );
                return ReloadReport {
                    ok: false,
                    errors: 1,
                    warnings,
                    listeners_rebound: false,
                };
            }
        }
    }
    tracing::info!("profile reloaded");
    ReloadReport {
        ok: true,
        errors: 0,
        warnings,
        listeners_rebound: rebound,
    }
}

/// What the system proxy should point at, given what is bound right now.
fn current_settings(
    engine: &Engine,
    listeners: &[(ListenerSpec, Running)],
) -> Option<ProxySettings> {
    let bound: Vec<_> = listeners
        .iter()
        .map(|(spec, running)| (spec.kind, running.local_addr))
        .collect();
    proxy_settings(&engine.runtime().config.general, &bound)
}

async fn switch_system_proxy(
    manager: &mut SystemProxyManager,
    engine: &Engine,
    listeners: &[(ListenerSpec, Running)],
    enabled: bool,
) -> Result<(), String> {
    if !enabled {
        return manager.disable().await;
    }
    let settings = current_settings(engine, listeners).ok_or_else(|| {
        "there is no http or socks5 listener to point the system proxy at".to_string()
    })?;
    manager.enable(settings).await
}

/// A reload may move the listeners or change `skip-proxy`: keep the system
/// proxy in step.
async fn reload_and_refresh(
    d: &Daemon<'_>,
    listeners: &mut Vec<(ListenerSpec, Running)>,
    sysproxy: &mut SystemProxyManager,
) -> ReloadReport {
    let report = reload(d, listeners).await;
    let settings = current_settings(d.engine, listeners);
    if report.ok
        && let Err(e) = sysproxy.refresh(settings.clone()).await
    {
        tracing::error!(error = %e, "cannot re-apply the system proxy after the reload");
    }
    // A reload that leaves no usable listener must not silently switch the
    // system proxy off: that would reroute the user's traffic direct without
    // them asking for it. Say so instead and leave the OS alone; a later
    // successful reload (or a stop) puts things right. A failed reload that
    // keeps the old listeners (a config-load or build error, not a rebind
    // failure) is not this case, so it logs nothing extra here.
    if settings.is_none()
        && let Some(applied) = sysproxy.applied()
    {
        tracing::warn!(
            "the system proxy still points at {} although rurge has no usable listener; reload again or stop rurge to restore it",
            describe(applied)
        );
    }
    report
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

/// Everything that means "shut down": Ctrl-C and SIGTERM on Unix; Ctrl-C,
/// the console closing, logoff and system shutdown on Windows (the last three
/// leave the process a few seconds — enough to put the system proxy back).
/// One long-lived stream per signal, built once: a stream buffers a signal
/// that arrives while nobody is awaiting it, whereas a fresh future per loop
/// iteration would drop every signal delivered during a reload.
struct ShutdownSignals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    close: tokio::signal::windows::CtrlClose,
    #[cfg(windows)]
    logoff: tokio::signal::windows::CtrlLogoff,
    #[cfg(windows)]
    shutdown: tokio::signal::windows::CtrlShutdown,
}

impl ShutdownSignals {
    fn new() -> anyhow::Result<ShutdownSignals> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(ShutdownSignals {
                interrupt: signal(SignalKind::interrupt()).context("cannot listen for SIGINT")?,
                terminate: signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?,
            })
        }
        #[cfg(windows)]
        {
            use tokio::signal::windows;
            Ok(ShutdownSignals {
                ctrl_c: windows::ctrl_c().context("cannot listen for Ctrl-C")?,
                close: windows::ctrl_close().context("cannot listen for the console closing")?,
                logoff: windows::ctrl_logoff().context("cannot listen for logoff")?,
                shutdown: windows::ctrl_shutdown().context("cannot listen for system shutdown")?,
            })
        }
    }

    async fn recv(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
        }
        #[cfg(windows)]
        tokio::select! {
            _ = self.ctrl_c.recv() => {}
            _ = self.close.recv() => {}
            _ = self.logoff.recv() => {}
            _ = self.shutdown.recv() => {}
        }
    }
}

/// One `rurge run` per data directory. The operating system drops the lock
/// when the process dies, so a crashed run never blocks the next start — and
/// a backup found in `state.json` while we hold it really is a dead run's.
///
/// Only a lock somebody else holds refuses the start: on a filesystem without
/// lock support, or a directory we cannot write, rurge warns and runs unlocked
/// rather than losing the daemon over it.
fn lock_data_dir(dir: &Path) -> Result<Option<File>, String> {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
    {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!(error = %e, "cannot open the data directory lock; running without the single-instance check");
            return Ok(None);
        }
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Err(format!(
            "another rurge instance is already running with the data directory {}",
            dir.display()
        )),
        Err(TryLockError::Error(e)) => {
            tracing::warn!(error = %e, "cannot lock the data directory; running without the single-instance check");
            Ok(None)
        }
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
    let (_log_guard, level_handle) = init_logging(
        args.log_level
            .unwrap_or_else(|| level_for(&cfg.general.loglevel)),
        args.log_file.as_deref(),
    )?;
    let rt = args.runtime.resolve(&cfg)?;
    // Named, so the lock lives until `run` returns — past the shutdown restore.
    let _instance_lock = match lock_data_dir(&rt.data_dir) {
        Ok(lock) => lock,
        Err(message) => {
            eprintln!("error: {message}");
            return Ok(ExitCode::from(1));
        }
    };
    let run_opts = RunOptions {
        idle_timeout: Duration::from_secs(args.idle_timeout.unwrap_or(600).max(1)),
        request_log_size: args.request_log_size.unwrap_or(1000).max(1),
    };
    let http_api = cfg.general.http_api.clone();
    // `cfg` is moved into the runtime below, so collect the watch list first.
    let cfg_paths: Vec<PathBuf> = std::iter::once(cfg.source.main.clone())
        .chain(cfg.source.includes.iter().cloned())
        .collect();
    let explicit_mode = args.outbound_mode.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let (store, state) = StateStore::open(rt.data_dir.join(STATE_FILE)).await;
        // Crash recovery first, before anything that can exit: a previous run
        // may have died with the operating system pointing at it, and the port
        // it left behind may be exactly why this start fails to bind. Putting
        // the pre-crash settings back is right whether or not this run starts.
        // We hold the data directory lock by now, so a backup left in
        // `state.json` belongs to a dead run, never to a live instance.
        let mut sysproxy = SystemProxyManager::new(super::sysproxy::backend()?, store.clone());
        sysproxy.recover().await;
        let outbound_mode = initial_mode(explicit_mode, &store, &state).await;
        // `cfg` is moved into `build_engine_runtime` next, so read its source path now.
        let shared = EngineShared::new(state.selections_for(&profile_key(&cfg.source.main)));
        let engine_rt =
            build_engine_runtime(cfg, &rt, &run_opts, outbound_mode.clone(), &shared).await?;
        print_diagnostics(engine_rt.diagnostics());
        let (policies, rules) = (
            engine_rt.policies.names().len(),
            engine_rt.rules.rules().len(),
        );
        let engine = Engine::new(engine_rt);
        engine.attach_state(store.clone());
        // a global policy saved by an earlier run (mode may be rule today)
        if engine.global_policy().is_none()
            && let Some(saved) = state.global_policy.clone()
            && let Err(e) = engine.set_global_policy(&saved).await
        {
            tracing::warn!(error = %e, "state.json names a global policy that is not in the profile; ignoring it");
        }
        engine.start_sampler();
        let mut listeners = match engine.bind_listeners().await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: cannot bind listener: {e}");
                return Ok(ExitCode::from(1));
            }
        };
        print_listening(&listeners);

        // Command channel from the API (M4 design §6). `cmd_tx` stays alive
        // here for the same reason as `reload_tx` below.
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(4);
        let control: Arc<dyn Control> = Arc::new(LoopControl {
            tx: cmd_tx.clone(),
            log_level: level_handle,
            system_proxy: sysproxy.flag(),
        });
        // The API comes up right after the listeners and before the summary
        // line, so `api on` is the third startup line (the CLI tests and
        // `rurge status` users read it there).
        let api_token = CancellationToken::new();
        if let Some(api) = http_api.clone() {
            let ctx = ApiContext {
                engine: engine.clone(),
                control: control.clone(),
                load_options: opts.clone(),
            };
            match rurge_api::serve(api.addr, api.key.clone(), ctx, api_token.clone()).await {
                Ok((addr, server)) => {
                    engine.tracker().spawn(server);
                    println!("api on http://{addr}");
                    tracing::info!(%addr, "http-api listening");
                }
                Err(e) => {
                    eprintln!("error: cannot bind http-api on {}: {e}", api.addr);
                    return Ok(ExitCode::from(1));
                }
            }
        }
        // Reload triggers. `reload_tx` stays alive here on purpose: were every
        // sender dropped, `recv()` would return `None` at once and the loop
        // below would spin reloading.
        let (reload_tx, mut reload_rx) = tokio::sync::mpsc::channel::<()>(1);
        let _watcher = if args.watch {
            Some(spawn_watcher(&cfg_paths, reload_tx.clone())?)
        } else {
            None
        };
        let mut signals = ShutdownSignals::new()?;
        #[cfg(unix)]
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .context("cannot listen for SIGHUP")?;

        // Recovery already ran, above. The enable waits until here because
        // everything fallible is behind us: nothing below returns early while
        // the operating system points at rurge, except the failure to enable.
        if args.system_proxy {
            if let Err(e) = switch_system_proxy(&mut sysproxy, &engine, &listeners, true).await {
                eprintln!("error: cannot enable the system proxy: {e}");
                return Ok(ExitCode::from(1));
            }
            let applied = sysproxy.applied().map(describe).unwrap_or_default();
            println!("system proxy enabled: {applied}");
            tracing::info!(%applied, "system proxy enabled");
        }

        // The one startup line that also reaches `--log-file` (the per-listener
        // "listening" records are DEBUG; stdout gets the lines above).
        tracing::info!(policies, rules, mode = %mode_name(&outbound_mode), "rurge running");
        println!(
            "rurge {} running: {policies} policies, {rules} rules, outbound mode {}",
            env!("CARGO_PKG_VERSION"),
            mode_name(&outbound_mode)
        );
        let daemon = Daemon {
            engine: &engine,
            config: &args.config,
            load_opts: &opts,
            rt: &rt,
            run_opts: &run_opts,
            outbound_mode: &outbound_mode,
            store: &store,
            http_api: &http_api,
        };

        loop {
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
                _ = signals.recv() => break,
                _ = reload_signal => {
                    reload_and_refresh(&daemon, &mut listeners, &mut sysproxy).await;
                }
                cmd = cmd_rx.recv() => match cmd {
                    Some(Command::Reload(reply)) => {
                        let report =
                            reload_and_refresh(&daemon, &mut listeners, &mut sysproxy).await;
                        let _ = reply.send(report);
                    }
                    Some(Command::Stop) => {
                        println!("stop requested via http-api");
                        break;
                    }
                    Some(Command::SystemProxy(enabled, reply)) => {
                        let result =
                            switch_system_proxy(&mut sysproxy, &engine, &listeners, enabled).await;
                        if let Err(e) = &result {
                            tracing::error!(error = %e, enabled, "cannot switch the system proxy");
                        }
                        let _ = reply.send(result);
                    }
                    None => {}
                },
            }
        }

        #[cfg(unix)]
        println!("shutting down (Ctrl-C or SIGTERM again to exit now)");
        #[cfg(not(unix))]
        println!("shutting down (Ctrl-C again to exit now)");
        // The system proxy first: rurge is about to stop serving, and the
        // forced exit below must not leave the OS pointing at a dead port.
        if sysproxy.enabled() {
            match sysproxy.disable().await {
                Ok(()) => println!("system proxy restored"),
                Err(e) => eprintln!(
                    "error: cannot restore the system proxy: {e}; the saved settings stay in state.json and are restored on the next start"
                ),
            }
        }
        // then the API, so its graceful stop runs inside the drain
        api_token.cancel();
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
        let force_exit = signals.recv();
        tokio::select! {
            _ = &mut drain => {}
            _ = force_exit => {
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

    #[test]
    fn the_data_dir_lock_is_exclusive_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let first = lock_data_dir(dir.path()).expect("the first start takes the lock");
        assert!(first.is_some(), "the lock file is lockable");
        let err = lock_data_dir(dir.path()).unwrap_err();
        assert!(err.contains("another rurge instance"), "{err}");
        drop(first);
        assert!(
            lock_data_dir(dir.path())
                .expect("the lock is free again")
                .is_some()
        );
    }
}
