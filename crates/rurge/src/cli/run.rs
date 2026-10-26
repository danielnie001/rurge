//! `rurge run`: the foreground proxy daemon (M3 design §9.3).

use super::enhanced::{EnhancedMode, wanted::Wanted};
use super::network;
use super::rule::{parse_mode, print_diagnostics};
use super::runtime::{PlatformOwners, PlatformProcesses, RuntimeArgs, owners_backend};
use super::switch;
use super::sysproxy::{SystemProxyManager, describe, proxy_settings};
use crate::capabilities;
use anyhow::Context;
use clap::Args;
use rurge_api::ApiContext;
use rurge_config::config::{LoadError, LoadOptions, Loaded, Platform};
use rurge_config::general::ControllerAccess;
use rurge_config::general::LogLevel;
use rurge_config::general::{Listener, parse_http_listen, parse_socks5_listen};
use rurge_config::rule::PolicyRef;
use rurge_config::session::ListenerKind;
use rurge_engine::control::{Control, EnhancedStatus, LogLevel as ApiLogLevel, Mode, ReloadReport};
use rurge_engine::profile::{Boot, BootLimits, ProfileSources, boot_need, disk_remote};
use rurge_engine::stack::DnsOverride;
use rurge_engine::state::{STATE_FILE, State, StateStore, profile_key};
use rurge_engine::{
    EmptyGroup, Engine, EngineShared, ListenerSpec, Running, Runtime, RuntimeOptions,
};
use rurge_net::BoxFuture;
use rurge_platform::sysproxy::ProxySettings;
use rurge_rules::OutboundMode;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::IsTerminal;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::Registry;
use tracing_subscriber::filter::{LevelFilter, Targets};
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
    #[arg(long, env = "RURGE_WATCH", value_parser = switch())]
    pub watch: bool,
    /// Point the operating system's proxy settings at rurge while it runs
    #[arg(long, env = "RURGE_SYSTEM_PROXY", value_parser = switch())]
    pub system_proxy: bool,
    /// Take the machine's traffic through a virtual adapter while rurge runs
    /// (the enhanced mode). Needs root or Administrator.
    #[arg(long, env = "RURGE_ENHANCED_MODE", value_parser = switch())]
    pub enhanced_mode: bool,
    /// A policy group without members rejects instead of going DIRECT
    #[arg(long, env = "RURGE_EMPTY_GROUP_REJECT", value_parser = switch())]
    pub empty_group_reject: bool,
    /// Listen for HTTP proxy clients at these addresses, in place of every
    /// http-listen address of the profile: [password@]address[:port], comma
    /// separated. A password given as the option shows in the process list;
    /// RURGE_LISTEN_HTTP keeps it out of it
    #[arg(
        long,
        env = "RURGE_LISTEN_HTTP",
        hide_env_values = true,
        value_name = "ADDR"
    )]
    pub listen_http: Option<String>,
    /// Listen for SOCKS5 clients at these addresses, in place of every
    /// socks5-listen address of the profile: address[:port], comma separated
    #[arg(
        long,
        env = "RURGE_LISTEN_SOCKS5",
        hide_env_values = true,
        value_name = "ADDR"
    )]
    pub listen_socks5: Option<String>,
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

/// What is not rurge's own says only what went wrong. A library's account
/// of its own work is written for whoever debugs the library, and has what
/// rurge keeps out of its log: an HTTP client names each connection by
/// where its requests go, a WebSocket library prints its frames with what
/// they carry, russh narrates every session down to what the server sent.
/// rurge says what comes of each. Two say nothing at all: boringtun's
/// handshake and timer messages carry no policy name, and the `wireguard`
/// outbound logs its handshakes; quinn's are about its drivers and every
/// packet, and the QUIC outbounds report what comes of them. quinn-udp's
/// warnings stay: what the system refused to send, which it drops without
/// telling anyone else.
fn quiet_dependencies() -> Targets {
    Targets::new()
        // a `log` record's target is its module, as an event's is
        .with_default(LevelFilter::WARN)
        // by prefix: `rurge` itself and every `rurge_…` crate
        .with_target("rurge", LevelFilter::TRACE)
        .with_target("boringtun", LevelFilter::OFF)
        // `quinn` covers `quinn_proto` too; the longer target wins
        .with_target("quinn", LevelFilter::OFF)
        .with_target("quinn_udp", LevelFilter::WARN)
}

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
        .with(quiet_dependencies())
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

/// Where `--listen-http` and `--listen-socks5` say to listen, read. Each
/// replaces the profile's whole list of its kind, so the profile's own
/// addresses are never bound while one is given. A `Listener` shows its
/// password as `<redacted>`, so this can be printed.
#[derive(Debug)]
struct Overrides {
    http: Option<Vec<Listener>>,
    socks5: Option<Vec<Listener>>,
}

impl Overrides {
    /// Reads the two options. An error is what to print after `error: `: the
    /// option and the reason, never the value, which can hold a password.
    fn parse(http: Option<&str>, socks5: Option<&str>) -> Result<Overrides, String> {
        let http = http
            .map(parse_http_listen)
            .transpose()
            .map_err(|reason| format!("--listen-http: {reason}"))?;
        let socks5 = socks5
            .map(parse_socks5_listen)
            .transpose()
            .map_err(|reason| format!("--listen-socks5: {reason}"))?;
        Ok(Overrides { http, socks5 })
    }
}

/// Puts the overrides over a profile that has just been loaded. A start and
/// every reload do it before they build anything from the profile, so that a
/// reload compares two configurations that both carry the overrides: editing
/// `http-listen` in the profile while `--listen-http` is given rebinds nothing.
///
/// The load keeps an `external` policy's `local-port` off the listeners the
/// profile writes, and the overrides come after it: rurge listening where the
/// program is to would dial itself, round and round. So the question is asked
/// again of the listeners as they are now. The error is what to print after
/// `error: `, as `Overrides::parse`'s is: the option and the reason, and not
/// the value.
fn apply(cfg: &mut rurge_config::Config, overrides: &Overrides) -> Result<(), String> {
    if let Some(http) = &overrides.http {
        cfg.general.http_listen = http.clone();
    }
    if let Some(socks5) = &overrides.socks5 {
        cfg.general.socks5_listen = socks5.clone();
    }
    match cfg.external_on_own_listener() {
        None => Ok(()),
        Some((key, port)) => {
            let option = match key {
                "http-listen" => "--listen-http",
                _ => "--listen-socks5",
            };
            Err(format!(
                "{option}: port {port} is the `local-port` of an `external` policy"
            ))
        }
    }
}

/// What the API can ask the main loop to do (M4 design §6), and the network
/// task (phase 3 M1 design 4): one at a time, in the order they come.
enum Command {
    Reload(oneshot::Sender<ReloadReport>),
    Stop,
    SystemProxy(bool, oneshot::Sender<Result<(), String>>),
    EnhancedMode(bool, oneshot::Sender<Result<(), String>>),
    /// The TUN of the open numbered so failed (`EnhancedMode::is_current_open`
    /// says whether that open is the one that is on).
    TunEnded(u64),
    /// The machine is on another network (`network::spawn` tells).
    NetworkChanged(network::NetworkChange),
}

struct LoopControl {
    tx: mpsc::Sender<Command>,
    log_level: LevelHandle,
    /// Mirrors `SystemProxyManager::enabled`.
    system_proxy: Arc<AtomicBool>,
    /// What the enhanced mode is, and what its TUN has counted.
    enhanced: Arc<super::enhanced::Shared>,
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

    fn set_enhanced_mode(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            self.tx
                .send(Command::EnhancedMode(enabled, reply))
                .await
                .map_err(|_| "rurge is shutting down".to_string())?;
            rx.await
                .unwrap_or_else(|_| Err("rurge is shutting down".to_string()))
        })
    }

    fn enhanced_mode(&self) -> EnhancedStatus {
        self.enhanced.status()
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
    http_api: &'a Option<ControllerAccess>,
    overrides: &'a Overrides,
    /// The downloaded sources of the generation in use, which a reload hands
    /// the sources of the next one.
    sources: &'a ProfileSources,
}

fn count(diags: &rurge_config::Diagnostics, severity: rurge_config::Severity) -> usize {
    diags.iter().filter(|d| d.severity == severity).count()
}

/// Reloads the profile from disk and swaps it in (M3b design §7.4). Every
/// failure path keeps the running config, so a bad edit never takes the
/// daemon down.
async fn reload(d: &Daemon<'_>, listeners: &mut Vec<(ListenerSpec, Running)>) -> ReloadReport {
    use rurge_config::Severity;
    let mut loaded = match rurge_engine::load_checked(d.config, d.load_opts) {
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
        // A profile that names a source nobody has downloaded cannot load
        // until the copy is here, and a load that fails registers nothing:
        // without this every later reload fails the same way.
        let followed = d.sources.follow_missing(&loaded.config.source);
        if followed > 0 {
            tracing::info!(
                sources = followed,
                "the profile on disk names sources that were never downloaded; they are being downloaded, and the profile is loaded again when one arrives"
            );
        }
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
    if let Err(e) = apply(&mut loaded.config, d.overrides) {
        eprintln!("error: reload failed, keeping current config: {e}");
        return failed_report();
    }
    // taken before the configuration moves into the runtime
    let source = loaded.config.source.clone();
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
    // The generation in use is the one whose sources are followed. A reload
    // that failed above keeps the configuration and the tracking it had, with
    // the sources its load named that nobody had downloaded added to it; the
    // generation that comes next decides whether those stay.
    d.sources.track(&source);
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

/// Whether the line of `[SSID Setting]` that the network the engine looks at
/// matches says `suspend=true` (phase 3 M1 design 7.4).
fn takeover_paused_here(engine: &Engine) -> bool {
    engine
        .runtime()
        .config
        .effective_ssid_setting(&engine.network())
        .is_some_and(|line| line.suspend)
}

/// What the enhanced mode should be for the generation in use and the network
/// (M4 design 4.6, 4.7): `ipv6_usable` is whether this network has IPv6 for
/// `ipv6-vif = auto` (P18). The start, the API's `POST`, a reload and a change
/// of network all ask it.
fn wanted_now(engine: &Engine, ipv6_usable: bool) -> Wanted {
    let runtime = engine.runtime();
    let general = &runtime.config.general;
    let external = runtime.config.external_addresses();
    let primary = engine
        .network()
        .primary
        .as_ref()
        .map(|primary| primary.interface.clone());
    super::enhanced::wanted::wanted(
        general,
        &external,
        ipv6_usable,
        primary,
        cfg!(target_os = "macos"),
    )
}

/// The system proxy and the enhanced mode as the user wants them, and as the
/// network lets them be (phase 3 M1 design 7.4, M4 design 9.9). While the
/// network says to leave the system alone the takeover is paused: both are
/// given back as they were, and what is asked for meanwhile is only noted. It
/// is in memory: `state.json` has them as they are, which is how a crash is
/// recovered from.
///
/// Not the engine's `suspend`, which refuses new sessions for a managed
/// profile whose update failed.
struct Takeover {
    /// What the user asked for last: `--system-proxy`, then what each
    /// `POST /v1/features/system_proxy` that was answered with success said.
    wanted: bool,
    /// The same for the enhanced mode: `--enhanced-mode`, then what each
    /// `POST /v1/features/enhanced_mode` that was answered with success said.
    /// A close that answers 500 has still taken the mode off, and is recorded
    /// as off too (the phase 3 / M4a plan's execution record row 97).
    enhanced_wanted: bool,
    /// The network says to leave the system alone.
    paused: bool,
}

impl Takeover {
    fn new(wanted: bool, enhanced_wanted: bool) -> Takeover {
        Takeover {
            wanted,
            enhanced_wanted,
            paused: false,
        }
    }

    /// The start: whether the network the daemon begins on pauses the takeover.
    /// If it does nothing is set, and `wanted` and `enhanced_wanted` are what
    /// the pause ends with.
    fn begin(&mut self, engine: &Engine) -> bool {
        self.paused = takeover_paused_here(engine);
        if self.paused {
            tracing::info!("takeover paused on this network");
        }
        self.paused
    }

    /// Whether the takeover is paused is the generation in use and the network
    /// together: look again after either has changed, and do what the
    /// difference asks. Entering the pause gives both back (that they were on
    /// is what the user wants, and is noted); leaving it makes both what the
    /// user last asked for, in both directions: pointed at rurge again if they
    /// want it so, and given back if they asked for that meanwhile and it is
    /// still on. That last part is the system proxy's case alone: a close of the
    /// enhanced mode never stops halfway, so the pause always gives it back and
    /// there is nothing of it left to give. Each is tried once.
    ///
    /// The enhanced mode goes first, in both directions, as it does at the exit
    /// (M4-D16): what it leaves behind is the machine's DNS.
    async fn follow(
        &mut self,
        engine: &Engine,
        listeners: &[(ListenerSpec, Running)],
        sysproxy: &mut SystemProxyManager,
        enhanced: &mut EnhancedMode,
        ipv6_usable: bool,
    ) {
        let paused = takeover_paused_here(engine);
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        if paused {
            if enhanced.enabled() {
                self.enhanced_wanted = true;
                if let Err(e) = enhanced.disable().await {
                    tracing::error!(
                        error = %e,
                        "the enhanced mode is off for the pause, but part of it could not be undone; what outlives rurge stays in state.json and is undone on the next start"
                    );
                }
            }
            if sysproxy.enabled() {
                self.wanted = true;
                if let Err(e) = sysproxy.disable().await {
                    tracing::error!(
                        error = %e,
                        "cannot give the system proxy back for the pause; it stays pointed at rurge"
                    );
                }
            }
            tracing::info!("takeover paused on this network");
        } else {
            // Only the direction that opens: a mode given back for the pause is
            // off whatever the user last asked for. `(false, true)` is the
            // system proxy's case alone, whose close can fail and leave it
            // pointed at rurge (the phase 3 / M4a plan's execution record row
            // 97).
            if self.enhanced_wanted
                && !enhanced.enabled()
                && let Err(e) = enhanced.enable(wanted_now(engine, ipv6_usable)).await
            {
                tracing::error!(
                    error = %e,
                    "cannot turn the enhanced mode on when the pause ends; it stays off"
                );
            }
            match (self.wanted, sysproxy.enabled()) {
                (true, false) => {
                    if let Err(e) = switch_system_proxy(sysproxy, engine, listeners, true).await {
                        tracing::error!(error = %e, "cannot point the system proxy at rurge again");
                    }
                }
                (false, true) => {
                    if let Err(e) = sysproxy.disable().await {
                        tracing::error!(
                            error = %e,
                            "cannot give the system proxy back when the pause ends; it stays pointed at rurge"
                        );
                    }
                }
                _ => {}
            }
            tracing::info!("takeover resumed");
        }
    }

    /// `POST /v1/features/system_proxy`. While the takeover is paused nothing
    /// is set: success is answered, and what was asked for is what is done
    /// when the pause ends.
    async fn switch(
        &mut self,
        engine: &Engine,
        listeners: &[(ListenerSpec, Running)],
        sysproxy: &mut SystemProxyManager,
        enabled: bool,
    ) -> Result<(), String> {
        if self.paused {
            tracing::info!(
                enabled,
                "the takeover is paused on this network; the system proxy is left as it is, and set as asked when the pause ends"
            );
        } else {
            switch_system_proxy(sysproxy, engine, listeners, enabled).await?;
        }
        self.wanted = enabled;
        Ok(())
    }

    /// `POST /v1/features/enhanced_mode`. While the takeover is paused nothing
    /// is opened or closed: success is answered, and what was asked for is what
    /// is done when the pause ends.
    async fn switch_enhanced(
        &mut self,
        engine: &Engine,
        enhanced: &mut EnhancedMode,
        ipv6_usable: bool,
        enabled: bool,
    ) -> Result<(), String> {
        if self.paused {
            tracing::info!(
                enabled,
                "the takeover is paused on this network; the enhanced mode is left as it is, and set as asked when the pause ends"
            );
        } else if enabled {
            enhanced.enable(wanted_now(engine, ipv6_usable)).await?;
        } else {
            // A close never stops halfway: it takes the mode off first, and an
            // error is one of its undos that did not go. What the user asked
            // for is off all the same, even though the error goes back to the
            // API's 500 (the phase 3 / M4a plan's execution record row 97).
            let result = enhanced.disable().await;
            self.enhanced_wanted = false;
            result?;
        }
        self.enhanced_wanted = enabled;
        Ok(())
    }
}

/// The machine is on another network (phase 3 M1 design 4): the engine is
/// told, a generation is built again if the DNS this network sets is not the
/// DNS the one in use was built with, and the takeover is paused or resumed if
/// the network says so. One at a time with the reloads and the commands of the
/// API, in the main loop.
async fn network_changed(
    d: &Daemon<'_>,
    change: network::NetworkChange,
    listeners: &[(ListenerSpec, Running)],
    sysproxy: &mut SystemProxyManager,
    takeover: &mut Takeover,
    enhanced: &mut EnhancedMode,
    ipv6_usable: &mut bool,
) {
    let network::NetworkChange {
        facts,
        ipv6_usable: on_this_network,
    } = change;
    // The interface and its kind are said at INFO. What says where the
    // machine is (the name of the network, the access point, the gateways) is
    // said at DEBUG only.
    let (interface, kind) = facts.primary.as_ref().map_or(("none", "none"), |primary| {
        (
            primary.interface.as_str(),
            match primary.kind {
                rurge_config::NetKind::Wifi => "wifi",
                rurge_config::NetKind::Wired => "wired",
                rurge_config::NetKind::Other => "other",
            },
        )
    });
    tracing::info!(
        interface,
        kind,
        wifi = facts.wifi.is_some(),
        "network changed"
    );
    tracing::debug!(
        ssid = ?facts.wifi.as_ref().and_then(|wifi| wifi.ssid.as_deref()),
        bssid = ?facts.wifi.as_ref().and_then(|wifi| wifi.bssid.as_deref()),
        routers = ?facts.primary.as_ref().map(|primary| &primary.routers),
        "where the machine is now"
    );
    // The engine reads the DNS servers of the system, which can take a moment.
    let engine = d.engine.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || engine.network_changed(facts)).await {
        tracing::error!(
            panicked = e.is_panic(),
            "cannot tell the engine that the network changed"
        );
    }
    follow_the_dns_of_the_network(d).await;
    // Before `takeover.follow`, which opens the enhanced mode when the pause
    // ends: what it opens with is this network's IPv6, not the last one's.
    *ipv6_usable = on_this_network;
    let was_on = enhanced.enabled();
    takeover
        .follow(d.engine, listeners, sysproxy, enhanced, *ipv6_usable)
        .await;
    // A mode `takeover.follow` has just opened is on this generation and this
    // network already: following again would only take the DNS and the
    // adjustments over once more.
    if was_on && enhanced.enabled() {
        enhanced
            .follow(wanted_now(d.engine, *ipv6_usable), true)
            .await;
    }
}

/// A generation is built with the DNS that `[SSID Setting]` sets for the
/// network of the moment (phase 3 M1 design 7.3). When the DNS the network sets
/// now is another one, the generation in use is built again from the
/// configuration it has in memory: not read again from the disk, for a network
/// that changes is no reason to bring in what the user has not asked to
/// reload. In-flight sessions and lookups finish on the generation they began
/// with. A generation that cannot be built is not put in the place of the one
/// in use, which goes on.
async fn follow_the_dns_of_the_network(d: &Daemon<'_>) {
    let runtime = d.engine.runtime();
    let wanted = DnsOverride::for_network(&runtime.config, &d.engine.network());
    if *runtime.dns_override() == wanted {
        return;
    }
    tracing::info!(
        "this network sets other DNS servers than the generation in use was built with; building it again"
    );
    let next = build_engine_runtime(
        (*runtime.config).clone(),
        d.rt,
        d.run_opts,
        d.outbound_mode.clone(),
        &d.engine.shared(),
    )
    .await;
    match next {
        Ok(next) => {
            // what a generation says when it is built, as at the start and in a
            // reload: the servers a line of `[SSID Setting]` lists are given to
            // a resolver only here, and a warning about one is said by nobody else
            print_diagnostics(next.diagnostics());
            // the configuration is the same: no listener has anything to change
            d.engine.swap_runtime(next);
        }
        Err(e) => tracing::error!(
            error = %format!("{e:#}"),
            "cannot build the generation again for the DNS of this network; the one in use goes on"
        ),
    }
}

/// A reload may move the listeners or change `skip-proxy`: keep the system
/// proxy in step. It may also add or take away the line of `[SSID Setting]`
/// that pauses the takeover.
async fn reload_and_refresh(
    d: &Daemon<'_>,
    listeners: &mut Vec<(ListenerSpec, Running)>,
    sysproxy: &mut SystemProxyManager,
    takeover: &mut Takeover,
    enhanced: &mut EnhancedMode,
    ipv6_usable: bool,
) -> ReloadReport {
    let report = reload(d, listeners).await;
    if report.ok {
        // A generation is built on the network the engine has, and only this
        // loop changes that, so this finds the DNS of the generation to be the
        // network's. It is the question asked after a change of network, asked
        // of whatever generation comes in.
        follow_the_dns_of_the_network(d).await;
        // A mode that is on takes the new generation with what differs (M4
        // design 9.7): the routes the profile's items and the external programs
        // add up to, the IPv6 half as `ipv6-vif` says, and the egress table. A
        // mode that is off stays off, and one `takeover.follow` has just opened
        // is on the new generation already.
        let was_on = enhanced.enabled();
        // before the refresh: it does nothing for a system proxy that is given back
        takeover
            .follow(d.engine, listeners, sysproxy, enhanced, ipv6_usable)
            .await;
        if was_on && enhanced.enabled() {
            enhanced
                .follow(wanted_now(d.engine, ipv6_usable), false)
                .await;
        }
    }
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

/// What a look at the sources asks of the engine.
#[derive(Debug, PartialEq, Eq)]
enum Ask {
    /// The engine is as the sources want it.
    Nothing,
    /// Refuse new sessions, for this reason; `say` is whether the log says
    /// so, which it does when the engine accepted a moment ago.
    Refuse { why: String, say: bool },
    /// Accept new sessions again.
    Accept,
}

/// What the sources ask of the engine, from what `strict` says (the reason
/// the update has failed), whether the update `strict` requires is due, and
/// why the engine refuses sessions, if it does.
fn what_the_sources_ask(strict: Option<&str>, due: bool, refusing: Option<&str>) -> Ask {
    match (strict, due, refusing) {
        (Some(why), _, None) => Ask::Refuse {
            why: why.to_string(),
            say: true,
        },
        (Some(why), _, Some(now)) if now == why => Ask::Nothing,
        (Some(why), _, Some(_)) => Ask::Refuse {
            why: why.to_string(),
            say: false,
        },
        // An update that is under way has not failed, and has not succeeded
        // either: whatever the engine does it goes on doing.
        (None, true, _) => Ask::Nothing,
        (None, false, Some(_)) => Ask::Accept,
        (None, false, None) => Ask::Nothing,
    }
}

/// The reason of a refusal that begins with a start whose update did not
/// finish, when the wait for the sources ended without one.
const UNFINISHED: &str = "the update has not finished";

/// Why the engine begins by refusing sessions, if it does: the update `strict`
/// requires has failed (its reason), or it is due and has not finished: then
/// with the reason the wait at the start ended with, which is the failure of
/// the same update, or with a sentence of its own when there is none.
fn suspended_at_start(strict: Option<&str>, due: bool, waited: Option<&str>) -> Option<String> {
    match strict {
        Some(why) => Some(why.to_string()),
        None if due => Some(waited.unwrap_or(UNFINISHED).to_string()),
        None => None,
    }
}

fn say_refusing(why: &str) {
    tracing::warn!(
        error = %why,
        "managed profile: the update that strict requires has not succeeded; new sessions are refused until it does"
    );
}

/// Makes the engine accept or refuse sessions as the sources ask. The log says
/// it when a refusal begins and when it ends, and not while it goes on.
fn follow_the_sources(sources: &ProfileSources, engine: &Engine) {
    let strict = sources.strict();
    let refusing = engine.suspended();
    match what_the_sources_ask(
        strict.as_ref().map(|suspended| suspended.why.as_str()),
        sources.strict_due(),
        refusing.as_deref(),
    ) {
        Ask::Nothing => {}
        Ask::Refuse { why, say } => {
            if say {
                say_refusing(&why);
            }
            engine.suspend(Some(why));
        }
        Ask::Accept => {
            engine.suspend(None);
            // An update that worked is one way out. The others update
            // nothing: a profile that no longer requires one, a clock that
            // was set back to before the last check, and a local file whose
            // time cannot be read when no check is on record.
            tracing::info!("managed profile: new sessions are accepted again");
        }
    }
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

/// What `state.json` says a dead run left behind. Read here rather than
/// through `StateStore`, which moves a file it cannot parse aside and says
/// so in the log: where this is asked there is no log, and without a backup
/// nothing there has any business with the file.
#[derive(Default)]
struct Left {
    system_proxy: bool,
    enhanced: bool,
}

impl Left {
    fn any(&self) -> bool {
        self.system_proxy || self.enhanced
    }
}

fn left_to_undo(state: &Path) -> Left {
    let Some(state) = std::fs::read_to_string(state)
        .ok()
        .and_then(|text| serde_json::from_str::<State>(&text).ok())
    else {
        return Left::default();
    };
    Left {
        system_proxy: state.system_proxy_backup.is_some(),
        enhanced: state.enhanced_backup.is_some(),
    }
}

/// Crash recovery for a start that ends before it has a log: at a profile
/// that does not load, or at a log file that cannot be opened. A run that
/// died with the system proxy pointing at it, or with the enhanced mode's
/// changes out in the world, is undone by the next start, but only by one that
/// gets as far as its own recovery: a start that ends here every time would
/// leave the machine pointing at a dead port, or its DNS at a responder that is
/// gone, until whatever stops it is repaired.
///
/// Unlike a start, this leaves a directory rurge keeps no state in exactly as
/// it finds it: the data directory is not created, and `state.json` is looked
/// for before the lock, so not even `rurge.lock` appears. What happened goes
/// to standard error, there being no log, and never changes how the start
/// ends.
fn recover_without_a_log(args: &RuntimeArgs) {
    let dir = args.data_dir();
    let path = dir.join(STATE_FILE);
    if !path.is_file() {
        return;
    }
    // Only with the lock in hand. Held by somebody else, the backup is that
    // live instance's to restore; and where no lock can be had at all,
    // nothing shows that there is no such instance. A start runs unlocked
    // there rather than not at all, but this one is ending anyway: the
    // backup waits for a start that gets further.
    let Ok(Some(_lock)) = lock_data_dir(&dir) else {
        return;
    };
    restore_what_a_crash_left(&path);
}

/// What `recover_without_a_log` does once it holds the lock: for a start that
/// holds it already, which cannot take it a second time. The enhanced mode
/// goes first: what it leaves behind is the machine's DNS.
fn restore_what_a_crash_left(path: &Path) {
    if !left_to_undo(path).any() {
        return;
    }
    restore_the_enhanced_mode(path);
    restore_the_system_proxy(path);
}

/// The enhanced mode's half of `restore_what_a_crash_left`.
fn restore_the_enhanced_mode(path: &Path) {
    if !left_to_undo(path).enhanced {
        return;
    }
    let backend = match super::enhanced::backend() {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("warning: {e}; the enhanced mode's changes were not undone");
            return;
        }
    };
    let recovered = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(runtime) => runtime.block_on(async {
            let (store, _) = StateStore::open(path.to_path_buf()).await;
            super::enhanced::EnhancedMode::recover(&backend, &store).await
        }),
        Err(e) => Some(Err(e.to_string())),
    };
    match recovered {
        // Forgetting a backup is a rewrite of `state.json`, and one that fails
        // reaches nothing but the log. A backup still there is undone again by
        // the next start, which is harmless: undoing twice is as undoing once.
        Some(Ok(())) if left_to_undo(path).enhanced => eprintln!(
            "warning: a previous run left changes of the enhanced mode; they were undone, but state.json could not be updated: the next start undoes them again"
        ),
        Some(Ok(())) => {
            eprintln!("warning: a previous run left changes of the enhanced mode; they were undone")
        }
        Some(Err(e)) => eprintln!(
            "warning: cannot undo what the enhanced mode left: {e}; it stays in state.json and is tried on the next start"
        ),
        // not under the lock: the store has just read the file looked at above
        None => {}
    }
}

/// The system proxy's half of `restore_what_a_crash_left`.
fn restore_the_system_proxy(path: &Path) {
    if !left_to_undo(path).system_proxy {
        return;
    }
    let backend = match super::sysproxy::backend() {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("warning: {e}; the system proxy was not restored");
            return;
        }
    };
    let recovered = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(runtime) => runtime.block_on(async {
            let (store, _) = StateStore::open(path.to_path_buf()).await;
            SystemProxyManager::new(backend, store).recover().await
        }),
        Err(e) => Some(Err(e.to_string())),
    };
    match recovered {
        // Forgetting the backup is a rewrite of `state.json`, and one that
        // fails reaches nothing but the log. A backup still there is restored
        // again by the next start, over whatever has been set since.
        Some(Ok(())) if left_to_undo(path).system_proxy => eprintln!(
            "warning: a previous run left the system proxy pointing at rurge; the saved settings were restored, but state.json could not be updated: the next start restores them again"
        ),
        Some(Ok(())) => eprintln!(
            "warning: a previous run left the system proxy pointing at rurge; the saved settings were restored"
        ),
        Some(Err(e)) => eprintln!(
            "warning: cannot restore the system proxy: {e}; the saved settings stay in state.json and are restored on the next start"
        ),
        // not under the lock: the store has just read the file looked at above
        None => {}
    }
}

/// Who holds the lock of the data directory while a start has no log yet.
enum Lock {
    /// Nobody: the start takes it once it has a log.
    NotYet,
    /// The start took it to download the sources of the profile, which write
    /// to the cache, before there was a log: `None` when no lock can be had
    /// at all, and the start goes on without one.
    Taken(Option<File>),
}

impl Lock {
    /// `recover_without_a_log` for whichever way the lock stands.
    fn recover(&self, args: &RuntimeArgs) {
        match self {
            Lock::NotYet => recover_without_a_log(args),
            Lock::Taken(Some(_)) => restore_what_a_crash_left(&args.data_dir().join(STATE_FILE)),
            // nothing shows that no live instance has the backup
            Lock::Taken(None) => {}
        }
    }
}

/// What the wait for the sources of the profile comes to.
struct Waited {
    /// The profile as it is by then. It can carry errors.
    loaded: Loaded,
    /// The lock the downloads were made under.
    lock: Lock,
    /// Why the update `strict` requires did not happen, when it did not.
    reason: Option<String>,
}

/// Downloads what the profile lacks or `strict` requires, before there is a
/// log. The downloads write to the cache in the data directory, which one
/// instance at a time may do: the lock is taken first and kept for the whole
/// run. `Break` is a start that ends without the profile; an error is one
/// that ends as a profile that cannot be read does.
fn wait_for_the_sources(
    args: &RunArgs,
    opts: &LoadOptions,
    first: Loaded,
    need: Boot,
    data_dir: &Path,
) -> anyhow::Result<ControlFlow<ExitCode, Waited>> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("cannot create data dir {}", data_dir.display()))?;
    let lock = match lock_data_dir(data_dir) {
        Ok(lock) => Lock::Taken(lock),
        Err(message) => {
            eprintln!("error: {message}");
            return Ok(ControlFlow::Break(ExitCode::from(1)));
        }
    };
    let limits = BootLimits::default();
    eprintln!(
        "waiting for the profile's remote sources (at most {} s)",
        limits.total.as_secs()
    );
    let waiting = || -> anyhow::Result<(Result<Loaded, LoadError>, Option<String>)> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let done = runtime.block_on(async {
            let sources = ProfileSources::production(&args.config, opts, data_dir, false)?;
            let loaded = sources.bootstrap(first, need, limits).await;
            // what the sources say of `strict` now is what the engine begins by
            let reason = sources.strict().map(|suspended| suspended.why);
            Ok::<_, anyhow::Error>((loaded, reason))
        });
        // The runtime is ended before anything else happens. One that runs on
        // this thread runs no task once `block_on` has returned: a download
        // still under way stops at the await it is at, and what a download
        // writes to the cache it writes between two awaits, in one piece.
        // What can still be running is on the blocking pool: the check of a
        // download, which is a load of the profile, or a name lookup. The
        // timeout waits for those. A lookup holds nothing of ours. A check
        // that is still at it after that, or queued behind one, goes on beside
        // the main runtime, and at its end it can write to the cache the
        // versions that were kept for the one it judged. The sources of the
        // main runtime are registered from what the disk holds and ask the
        // server again when they are due, so a late write is as a rule one
        // more update. The exception is a write that meets the main runtime's
        // own write of the same entry: the two go through the same temporary
        // file, and the body can end up with parts of both. It does not match
        // the digest that is kept beside it then, which makes the two files
        // no entry: whoever reads the disk finds none, and the next request
        // of the main runtime that succeeds writes the body again, whether
        // the server sends one or says that nothing has changed. A request
        // that fails writes nothing.
        runtime.shutdown_timeout(Duration::from_secs(2));
        done
    };
    let (loaded, reason) = match waiting() {
        Ok(done) => done,
        Err(e) => {
            lock.recover(&args.runtime);
            return Err(e);
        }
    };
    match loaded {
        Ok(loaded) => Ok(ControlFlow::Continue(Waited {
            loaded,
            lock,
            reason,
        })),
        Err(e) => {
            lock.recover(&args.runtime);
            Err(e.into())
        }
    }
}

pub fn run(args: RunArgs) -> anyhow::Result<ExitCode> {
    // Before the profile: the listen options are a command-line matter, and a
    // start that ends on one ends the way a profile that does not load does.
    let overrides =
        match Overrides::parse(args.listen_http.as_deref(), args.listen_socks5.as_deref()) {
            Ok(overrides) => overrides,
            Err(message) => {
                eprintln!("error: {message}");
                recover_without_a_log(&args.runtime);
                return Ok(ExitCode::from(2));
            }
        };
    // Where the network is read from is a matter of the option and the
    // variable behind it, and ends a start that cannot tell as an option that
    // cannot be read does: a typo must not make a start read the machine's
    // network.
    let network_backend = match network::backend(args.runtime.network_backend.as_deref()) {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("error: {e}");
            recover_without_a_log(&args.runtime);
            return Ok(ExitCode::from(2));
        }
    };
    // Whether the processes behind the sessions are looked up is a matter of a
    // variable too, and a value that names nothing ends a start as an unknown
    // network backend does: a typo must not make a start look at the machine's
    // own processes.
    let look_up_processes =
        match owners_backend(std::env::var_os("RURGE_PROCESS_BACKEND").as_deref()) {
            Ok(on) => on,
            Err(e) => {
                eprintln!("error: {e}");
                recover_without_a_log(&args.runtime);
                return Ok(ExitCode::from(2));
            }
        };
    // What the enhanced mode may touch is a matter of a variable as well, and a
    // value that names nothing ends a start as an unknown network backend
    // does: a typo must not let a start take over the machine's DNS.
    let enhanced_backend = match super::enhanced::backend() {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("error: {e}");
            recover_without_a_log(&args.runtime);
            return Ok(ExitCode::from(2));
        }
    };
    let platform = args.platform.unwrap_or_else(Platform::current);
    let data_dir = args.runtime.data_dir();
    let mut opts = LoadOptions {
        environment: super::environment(platform, capabilities::CORE_VERSION),
        platform,
        capabilities: capabilities::current(),
        // what an earlier run downloaded: a profile asks the disk about the
        // addresses it names, and nothing else is read or made
        remote: Some(disk_remote(&data_dir)),
    };
    // A profile that cannot be read ends the start here, and one that has
    // errors ends it below, once what it lacks has been waited for; in both,
    // what a crashed run left behind is put back on the way out.
    let first = rurge_engine::load_checked(&args.config, &opts)
        .inspect_err(|_| recover_without_a_log(&args.runtime))?;
    let need = if args.runtime.no_network {
        Boot::No
    } else {
        boot_need(&first, &data_dir)
    };
    // A profile whose sources are downloaded and not all there is waited for
    // before there is a log, and the profile that results is the one the
    // start goes on with. Anything else is the profile as the disk has it.
    let (loaded, lock, boot_reason) = if need == Boot::No {
        (first, Lock::NotYet, None)
    } else {
        match wait_for_the_sources(&args, &opts, first, need, &data_dir)? {
            ControlFlow::Break(code) => return Ok(code),
            ControlFlow::Continue(waited) => (waited.loaded, waited.lock, waited.reason),
        }
    };
    if loaded.diagnostics.has_errors() {
        print_diagnostics(&loaded.diagnostics.sorted());
        lock.recover(&args.runtime);
        return Ok(ExitCode::from(2));
    }
    print_diagnostics(&loaded.diagnostics.sorted());
    let mut cfg = loaded.config;
    // An override that puts rurge's own listener on the port of an `external`
    // policy ends the start as an option that cannot be read does.
    if let Err(message) = apply(&mut cfg, &overrides) {
        eprintln!("error: {message}");
        lock.recover(&args.runtime);
        return Ok(ExitCode::from(2));
    }
    // A log file that cannot be opened ends it as well, and in the same way.
    let (_log_guard, level_handle) = init_logging(
        args.log_level
            .unwrap_or_else(|| level_for(&cfg.general.loglevel)),
        args.log_file.as_deref(),
    )
    .inspect_err(|_| lock.recover(&args.runtime))?;
    let rt = args.runtime.resolve(&cfg)?;
    // Named, so the lock lives until `run` returns — past the shutdown restore.
    let _instance_lock = match lock {
        Lock::NotYet => match lock_data_dir(&rt.data_dir) {
            Ok(lock) => lock,
            Err(message) => {
                eprintln!("error: {message}");
                return Ok(ExitCode::from(1));
            }
        },
        // taken before the downloads, and not to be taken twice
        Lock::Taken(lock) => {
            if lock.is_none() {
                // the log did not exist when the lock was tried
                tracing::warn!(
                    "the data directory is not locked; running without the single-instance check"
                );
            }
            lock
        }
    };
    let run_opts = RunOptions {
        idle_timeout: Duration::from_secs(args.idle_timeout.unwrap_or(600).max(1)),
        request_log_size: args.request_log_size.unwrap_or(1000).max(1),
    };
    let http_api = cfg.general.http_api.clone();
    // `cfg` is moved into the runtime below, so collect the watch list first.
    // It is the files the user wrote: what is downloaded is kept in the data
    // directory and followed by the sources. The list is made now, so a file
    // added to the profile's own includes is watched from the next start on;
    // while the downloaded copy of a managed profile is in effect the list is
    // the local file alone, for the copy includes no local file.
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
        // `state.json` belongs to a dead run, never to a live instance. The
        // enhanced mode goes first: what it leaves behind is the machine's DNS.
        match super::enhanced::EnhancedMode::recover(&enhanced_backend, &store).await {
            Some(Ok(())) => tracing::info!("restored what the enhanced mode left last time"),
            Some(Err(e)) => tracing::error!(
                error = %e,
                "cannot undo what the enhanced mode left; it stays in state.json and is tried on the next start"
            ),
            None => {}
        }
        let mut sysproxy = SystemProxyManager::new(super::sysproxy::backend()?, store.clone());
        sysproxy.recover().await;
        let outbound_mode = initial_mode(explicit_mode, &store, &state).await;
        // The sources of the profile that are downloaded, which this run
        // follows. What a wait before the log downloaded is on the disk, and
        // that is all they need to know of it. Every reload, and the API's
        // check of the profile, read the profile as they do, which also says
        // why a download failed.
        let sources =
            ProfileSources::production(&args.config, &opts, &rt.data_dir, rt.no_network)?;
        opts.remote = Some(sources.remote());
        sources.track(&cfg.source);
        // `cfg` is moved into `build_engine_runtime` next, so read its source path now.
        let mut shared = EngineShared::new(state.selections_for(&profile_key(&cfg.source.main)));
        if args.empty_group_reject {
            shared.empty_group = EmptyGroup::Reject;
        }
        shared.processes = Arc::new(PlatformProcesses);
        if look_up_processes {
            shared.owners = Some(Arc::new(PlatformOwners::new()));
        }
        // The network before the first generation, which is built on it: its
        // `subnet` groups and `SUBNET` rules look at it from the first
        // session on, and the DNS `[SSID Setting]` sets for it is the DNS of
        // its resolver.
        let started = network::start(&network_backend).await;
        shared
            .network
            .store(network::facts_of(&started.snapshot));
        // Whether this network has IPv6 for `ipv6-vif = auto` (P18): the
        // start asks once, and every change of network asks again.
        let mut ipv6_usable = network::ipv6_usable(&started.snapshot);
        let engine_rt =
            build_engine_runtime(cfg, &rt, &run_opts, outbound_mode.clone(), &shared).await?;
        print_diagnostics(engine_rt.diagnostics());
        let engine = Engine::new(engine_rt);
        let (policies, rules) = (
            engine.registry().names().len(),
            engine.runtime().rules.rules().len(),
        );
        engine.attach_state(store.clone());
        // a global policy saved by an earlier run (mode may be rule today)
        if engine.global_policy().is_none()
            && let Some(saved) = state.global_policy.clone()
            && let Err(e) = engine.set_global_policy(&saved).await
        {
            tracing::warn!(error = %e, "state.json names a global policy that is not in the profile; ignoring it");
        }
        engine.start_sampler();
        // An update that `strict` requires and is owed by now: the engine
        // comes up refusing, from the first session on. The sources of this
        // run have made no attempt yet and cannot say that one failed, so the
        // wait before the log says it. A profile that falls due between that
        // wait and this run's first attempt is refused for the time of the
        // download, and accepted again when it succeeds.
        if let Some(why) = suspended_at_start(
            sources.strict().as_ref().map(|suspended| suspended.why.as_str()),
            sources.strict_due(),
            boot_reason.as_deref(),
        ) {
            say_refusing(&why);
            engine.suspend(Some(why));
        }
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
        // The enhanced mode's manager has to be here before the API: the API
        // reads its state. It is not opened until the listeners and the API
        // are up (P25, below).
        let mut enhanced = super::enhanced::EnhancedMode::new(
            enhanced_backend,
            store.clone(),
            super::enhanced::starter(engine.clone()),
            super::enhanced::egress_slot(),
            {
                let tx = cmd_tx.clone();
                Arc::new(move |sequence| {
                    // a failure must not be lost to a full queue: the send is
                    // its own task, and an already-finished loop drops it
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let _ = tx.send(Command::TunEnded(sequence)).await;
                    });
                })
            },
        );
        let control: Arc<dyn Control> = Arc::new(LoopControl {
            tx: cmd_tx.clone(),
            log_level: level_handle,
            system_proxy: sysproxy.flag(),
            enhanced: enhanced.shared(),
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
                sources: Some(sources.clone()),
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
        //
        // On a network that says to leave the system alone neither is set:
        // the options are only noted, and done when the pause ends (M4-D18).
        let mut takeover = Takeover::new(args.system_proxy, args.enhanced_mode);
        let paused = takeover.begin(&engine);
        // The enhanced mode goes first (P25): the system proxy points at the
        // listeners that are already up, and a start whose system proxy fails
        // gives the enhanced mode back before it exits.
        if !paused && args.enhanced_mode {
            match enhanced.enable(wanted_now(&engine, ipv6_usable)).await {
                Ok(()) => println!("enhanced mode on"),
                Err(e) => {
                    eprintln!("error: cannot turn the enhanced mode on: {e}");
                    return Ok(ExitCode::from(1));
                }
            }
        }
        // The system proxy comes next.
        if !paused && args.system_proxy {
            if let Err(e) = switch_system_proxy(&mut sysproxy, &engine, &listeners, true).await {
                eprintln!("error: cannot enable the system proxy: {e}");
                // P25: what this start did goes back before it ends
                if enhanced.enabled() {
                    match enhanced.disable().await {
                        Ok(()) => println!("enhanced mode off"),
                        Err(e) => eprintln!(
                            "error: cannot turn the enhanced mode off: {e}; what outlives rurge stays in state.json and is undone on the next start"
                        ),
                    }
                }
                return Ok(ExitCode::from(1));
            }
            let applied = sysproxy.applied().map(describe).unwrap_or_default();
            println!("system proxy enabled: {applied}");
            tracing::info!(%applied, "system proxy enabled");
        }

        // Nothing below can end the start: this is the profile that runs, and
        // a start that did not get here did not run its profile. `update`
        // rewrites the file, so a profile that is recorded already is not
        // written again.
        let profile = std::path::absolute(&args.config)
            .unwrap_or_else(|_| args.config.clone())
            .to_string_lossy()
            .into_owned();
        if state.current_profile.as_deref() != Some(profile.as_str()) {
            store.update(|s| s.current_profile = Some(profile)).await;
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
            http_api: &http_api,
            overrides: &overrides,
            sources: &sources,
        };

        // The sources are looked at once a second, and a look that is late is
        // not made up for.
        let mut look = tokio::time::interval(Duration::from_secs(1));
        look.set_missed_tick_behavior(MissedTickBehavior::Delay);

        // The network is followed from here on: what changes in it reaches
        // the loop below as a command, one at a time with the rest.
        let following_the_network = network::spawn(
            network_backend,
            started,
            cmd_tx.clone(),
            Command::NetworkChanged,
        );

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
                    reload_and_refresh(
                        &daemon,
                        &mut listeners,
                        &mut sysproxy,
                        &mut takeover,
                        &mut enhanced,
                        ipv6_usable,
                    )
                    .await;
                }
                // A source has a new version: the same reload as any other
                // trigger, and the only one made for a download. It comes a
                // quiet second after the version, and a refusal ends at the
                // next look, so for up to a second new sessions run on the
                // generation that was due.
                _ = sources.changed() => {
                    reload_and_refresh(
                        &daemon,
                        &mut listeners,
                        &mut sysproxy,
                        &mut takeover,
                        &mut enhanced,
                        ipv6_usable,
                    )
                    .await;
                }
                _ = look.tick() => {
                    follow_the_sources(&sources, &engine);
                    // the local file has been outlived by an update
                    if sources.outdated() {
                        reload_and_refresh(
                            &daemon,
                            &mut listeners,
                            &mut sysproxy,
                            &mut takeover,
                            &mut enhanced,
                            ipv6_usable,
                        )
                        .await;
                    }
                }
                cmd = cmd_rx.recv() => match cmd {
                    Some(Command::Reload(reply)) => {
                        let report = reload_and_refresh(
                            &daemon,
                            &mut listeners,
                            &mut sysproxy,
                            &mut takeover,
                            &mut enhanced,
                            ipv6_usable,
                        )
                        .await;
                        let _ = reply.send(report);
                    }
                    Some(Command::Stop) => {
                        println!("stop requested via http-api");
                        break;
                    }
                    Some(Command::SystemProxy(enabled, reply)) => {
                        let result = takeover
                            .switch(&engine, &listeners, &mut sysproxy, enabled)
                            .await;
                        if let Err(e) = &result {
                            tracing::error!(error = %e, enabled, "cannot switch the system proxy");
                        }
                        let _ = reply.send(result);
                    }
                    Some(Command::EnhancedMode(enabled, reply)) => {
                        // on a paused network this only notes the request (M4
                        // design 9.9)
                        let result = takeover
                            .switch_enhanced(&engine, &mut enhanced, ipv6_usable, enabled)
                            .await;
                        if let Err(e) = &result {
                            tracing::error!(error = %e, enabled, "cannot switch the enhanced mode");
                        }
                        let _ = reply.send(result);
                    }
                    Some(Command::TunEnded(sequence)) => {
                        // a failure of an open that has been closed, or of one
                        // an open has replaced since, is not this one's
                        if enhanced.is_current_open(sequence) {
                            tracing::error!("the virtual adapter failed; the enhanced mode is off");
                            if let Err(e) = enhanced.disable().await {
                                tracing::error!(
                                    error = %e,
                                    "the enhanced mode is off after the adapter failed, but part of it could not be undone; what outlives rurge stays in state.json and is undone on the next start"
                                );
                            }
                        }
                    }
                    Some(Command::NetworkChanged(change)) => {
                        network_changed(
                            &daemon,
                            change,
                            &listeners,
                            &mut sysproxy,
                            &mut takeover,
                            &mut enhanced,
                            &mut ipv6_usable,
                        )
                        .await;
                    }
                    None => {}
                },
            }
        }
        // nothing is told of the network any more
        drop(following_the_network);

        #[cfg(unix)]
        println!("shutting down (Ctrl-C or SIGTERM again to exit now)");
        #[cfg(not(unix))]
        println!("shutting down (Ctrl-C again to exit now)");
        // The enhanced mode first (M4-D16): what it leaves behind is the
        // machine's DNS, and the sessions on the adapter ended with its TUN.
        if enhanced.enabled() {
            match enhanced.disable().await {
                Ok(()) => println!("enhanced mode off"),
                Err(e) => eprintln!(
                    "error: cannot turn the enhanced mode off: {e}; what outlives rurge stays in state.json and is undone on the next start"
                ),
            }
        }
        // The system proxy next: rurge is about to stop serving, and the
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
                // no grace for the external programs either: ending the
                // runtime ends the tasks that hold them, which kill them
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
        // the sessions are over: the programs behind `external` policies go
        // last, each with what it started (phase 2 M4 design 7.3)
        engine.stop_external_programs().await;
        Ok(ExitCode::SUCCESS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boringtun_says_nothing() {
        let filter = quiet_dependencies();
        let error = &tracing::Level::ERROR;
        assert!(!filter.would_enable("boringtun::noise::timers", error));
        assert!(filter.would_enable("rurge_proto_wireguard::device", &tracing::Level::TRACE));
    }

    #[test]
    fn quinn_says_only_what_rurge_cannot() {
        let filter = quiet_dependencies();
        let error = &tracing::Level::ERROR;
        // its drivers and the packets of the protocol: the QUIC outbounds
        // report what comes of them
        assert!(!filter.would_enable("quinn", error));
        assert!(!filter.would_enable("quinn_proto::connection", error));
        // what the system refused to send, which nothing else sees
        assert!(filter.would_enable("quinn_udp", &tracing::Level::WARN));
        assert!(!filter.would_enable("quinn_udp::imp", &tracing::Level::DEBUG));
        assert!(filter.would_enable("rurge_proto_quic::session", &tracing::Level::TRACE));
    }

    #[test]
    fn russh_says_only_its_warnings() {
        let filter = quiet_dependencies();
        // its own account of a session, down to what the server sent: the
        // `ssh` outbound reports what comes of it
        assert!(!filter.would_enable("russh::client::encrypted", &tracing::Level::INFO));
        assert!(!filter.would_enable("russh::client", &tracing::Level::DEBUG));
        assert!(!filter.would_enable("russh::cipher", &tracing::Level::TRACE));
        assert!(!filter.would_enable("russh_cryptovec::platform::windows", &tracing::Level::DEBUG));
        // what went wrong inside it, which nothing else sees
        assert!(filter.would_enable("russh::client::encrypted", &tracing::Level::WARN));
        assert!(filter.would_enable("russh_cryptovec::platform", &tracing::Level::WARN));
        assert!(filter.would_enable("russh::client", &tracing::Level::ERROR));
        assert!(filter.would_enable("rurge_proto_ssh::outbound", &tracing::Level::TRACE));
    }

    /// What is not rurge's own says only what went wrong. An HTTP library's
    /// account of its connections names each by where it goes; a WebSocket
    /// library's account of its frames has what they carry.
    #[test]
    fn what_is_not_rurges_own_says_only_what_went_wrong() {
        let filter = quiet_dependencies();
        for target in [
            "hyper_util::client::legacy::pool",
            "hyper::proto::h1::conn",
            "h2::codec::framed_read",
            "rustls::client::hs",
            "tungstenite::protocol",
            "tokio_tungstenite",
            "a_crate_of_next_year::module",
        ] {
            for level in [
                tracing::Level::TRACE,
                tracing::Level::DEBUG,
                tracing::Level::INFO,
            ] {
                assert!(!filter.would_enable(target, &level), "{target} at {level}");
            }
            for level in [tracing::Level::WARN, tracing::Level::ERROR] {
                assert!(filter.would_enable(target, &level), "{target} at {level}");
            }
        }
        // rurge's own, down to the last line
        for target in [
            "rurge",
            "rurge::cli::run",
            "rurge_net::resource",
            "rurge_dns::fanout",
            "rurge_engine::profile",
        ] {
            assert!(
                filter.would_enable(target, &tracing::Level::TRACE),
                "{target}"
            );
        }
    }

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

    /// `POST /v1/log/level` sets, for each of its six names, the level the
    /// option of that name sets; `debug` is `info`, and `error` is the one
    /// level below `warning` that the profile's four names do not have.
    #[test]
    fn the_api_sets_the_level_the_option_of_the_same_name_does() {
        let (_layer, handle): (reload::Layer<LevelFilter, Registry>, LevelHandle) =
            reload::Layer::new(LevelFilter::OFF);
        let (tx, _commands) = mpsc::channel(1);
        let control = LoopControl {
            tx,
            log_level: handle.clone(),
            system_proxy: Arc::new(AtomicBool::new(false)),
            enhanced: Arc::new(crate::cli::enhanced::Shared::new()),
        };
        for (name, filter) in [
            ("verbose", LevelFilter::TRACE),
            ("debug", LevelFilter::DEBUG),
            ("info", LevelFilter::DEBUG),
            ("notify", LevelFilter::INFO),
            ("warning", LevelFilter::WARN),
            ("error", LevelFilter::ERROR),
        ] {
            let level = ApiLogLevel::parse(name).expect("a level the API knows");
            control.set_log_level(level).unwrap();
            assert_eq!(handle.with_current(|f| *f).unwrap(), filter, "{name}");
            assert_eq!(parse_log_level(name).unwrap(), filter, "{name}");
        }
    }

    fn profile(general: &str) -> rurge_config::Config {
        let text = format!("[General]\n{general}\n[Rule]\nFINAL,DIRECT\n");
        let loaded =
            rurge_config::config::from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        loaded.config
    }

    fn overrides(http: Option<&str>, socks5: Option<&str>) -> Overrides {
        Overrides::parse(http, socks5).expect("values to read")
    }

    fn addresses(listeners: &[Listener]) -> Vec<String> {
        listeners.iter().map(|l| l.addr.to_string()).collect()
    }

    const TWO_HTTP_AND_ONE_SOCKS5: &str =
        "http-listen = 127.0.0.1:7001, pw@127.0.0.1:7002\nsocks5-listen = 127.0.0.1:7003";

    /// An override replaces the profile's whole list of its kind, however long
    /// it is, and nothing else of the profile moves.
    #[test]
    fn an_http_override_replaces_every_http_listener_and_leaves_socks5_alone() {
        let mut cfg = profile(TWO_HTTP_AND_ONE_SOCKS5);
        let before = cfg.general.clone();
        assert_eq!(addresses(&before.http_listen).len(), 2);
        apply(&mut cfg, &overrides(Some("S3CR3T@127.0.0.1:9001"), None)).unwrap();
        assert_eq!(addresses(&cfg.general.http_listen), ["127.0.0.1:9001"]);
        assert_eq!(
            cfg.general.http_listen[0].password.as_deref(),
            Some("S3CR3T")
        );
        assert_eq!(cfg.general.socks5_listen, before.socks5_listen);
        let mut expected = before;
        expected.http_listen = cfg.general.http_listen.clone();
        assert_eq!(cfg.general, expected);
    }

    #[test]
    fn a_socks5_override_replaces_the_socks5_listeners_and_leaves_http_alone() {
        let mut cfg = profile(TWO_HTTP_AND_ONE_SOCKS5);
        let before = cfg.general.clone();
        apply(
            &mut cfg,
            &overrides(None, Some("[::1]:9003, 127.0.0.1:9004")),
        )
        .unwrap();
        assert_eq!(
            addresses(&cfg.general.socks5_listen),
            ["[::1]:9003", "127.0.0.1:9004"]
        );
        assert_eq!(cfg.general.http_listen, before.http_listen);
    }

    #[test]
    fn both_overrides_apply_and_none_leaves_the_profile_as_it_is() {
        let mut cfg = profile(TWO_HTTP_AND_ONE_SOCKS5);
        let before = cfg.general.clone();
        apply(&mut cfg, &overrides(None, None)).unwrap();
        assert_eq!(cfg.general, before);
        apply(
            &mut cfg,
            &overrides(Some("127.0.0.1:9001"), Some("127.0.0.1:9003")),
        )
        .unwrap();
        assert_eq!(addresses(&cfg.general.http_listen), ["127.0.0.1:9001"]);
        assert_eq!(addresses(&cfg.general.socks5_listen), ["127.0.0.1:9003"]);
        // applied to a profile read again, as a reload does, it gives the same
        let mut again = profile("http-listen = 127.0.0.1:7777");
        apply(
            &mut again,
            &overrides(Some("127.0.0.1:9001"), Some("127.0.0.1:9003")),
        )
        .unwrap();
        assert_eq!(again.general.http_listen, cfg.general.http_listen);
        assert_eq!(again.general.socks5_listen, cfg.general.socks5_listen);
    }

    /// A profile that loads, with its own listeners elsewhere and an
    /// `external` policy whose program is to listen on `port`.
    fn profile_with_an_external_policy(port: u16) -> rurge_config::Config {
        profile(&format!(
            "http-listen = 127.0.0.1:7001\nsocks5-listen = 127.0.0.1:7003\n\
[Proxy]\nX = external, exec=/no/such/program, local-port={port}"
        ))
    }

    /// The load keeps an `external` policy off the listeners the profile has,
    /// but the overrides come after it: rurge on the port the program is to
    /// listen on would dial itself. The message is the option and the reason,
    /// as for a value that cannot be read, and not the value.
    #[test]
    fn an_override_on_the_port_of_an_external_policy_is_refused() {
        let mut cfg = profile_with_an_external_policy(1080);
        assert_eq!(
            apply(&mut cfg, &overrides(None, Some("127.0.0.1:1080"))),
            Err(
                "--listen-socks5: port 1080 is the `local-port` of an `external` policy"
                    .to_string()
            )
        );
        let mut cfg = profile_with_an_external_policy(1080);
        let message = apply(&mut cfg, &overrides(Some("S3CR3T@127.0.0.1:1080"), None)).unwrap_err();
        assert_eq!(
            message,
            "--listen-http: port 1080 is the `local-port` of an `external` policy"
        );
        assert!(!message.contains("S3CR3T"), "{message}");
        // an address of the other family or on every interface is the port too
        for (http, socks5, option) in [
            (None, Some("[::1]:1080"), "--listen-socks5"),
            (None, Some("0.0.0.0:1080"), "--listen-socks5"),
            (Some("[::]:1080"), None, "--listen-http"),
            (
                Some("127.0.0.1:1081"),
                Some("127.0.0.1:1080"),
                "--listen-socks5",
            ),
        ] {
            let mut cfg = profile_with_an_external_policy(1080);
            let message = apply(&mut cfg, &overrides(http, socks5)).unwrap_err();
            assert!(
                message.starts_with(&format!("{option}: port 1080 ")),
                "{http:?} {socks5:?}: {message}"
            );
        }
    }

    #[test]
    fn an_override_on_another_port_is_taken_beside_an_external_policy() {
        let mut cfg = profile_with_an_external_policy(1080);
        apply(
            &mut cfg,
            &overrides(Some("127.0.0.1:1081"), Some("127.0.0.1:1082")),
        )
        .unwrap();
        assert_eq!(addresses(&cfg.general.http_listen), ["127.0.0.1:1081"]);
        assert_eq!(addresses(&cfg.general.socks5_listen), ["127.0.0.1:1082"]);
        // the program listens at loopback: an address elsewhere is not its port
        let mut cfg = profile_with_an_external_policy(1080);
        apply(&mut cfg, &overrides(None, Some("192.0.2.1:1080"))).unwrap();
        assert_eq!(addresses(&cfg.general.socks5_listen), ["192.0.2.1:1080"]);
        // and with no override there is nothing to ask
        let mut cfg = profile_with_an_external_policy(1080);
        let before = cfg.general.clone();
        apply(&mut cfg, &overrides(None, None)).unwrap();
        assert_eq!(cfg.general, before);
    }

    /// The error is for the person at the terminal: the option and the
    /// reason, and not the value, which can hold a password.
    #[test]
    fn a_listen_option_that_cannot_be_read_is_named_and_not_quoted() {
        for (http, socks5, option) in [
            (Some("S3CR3T@not-an-address"), None, "--listen-http"),
            (Some(""), None, "--listen-http"),
            (Some(","), None, "--listen-http"),
            (None, Some("S3CR3T@127.0.0.1:1080"), "--listen-socks5"),
            (None, Some("nowhere:1080"), "--listen-socks5"),
            // the first of two is the one that is named
            (Some("not-an-address"), Some("nowhere"), "--listen-http"),
        ] {
            let message = Overrides::parse(http, socks5).unwrap_err();
            assert!(message.starts_with(&format!("{option}: ")), "{message}");
            for value in ["S3CR3T", "not-an-address", "nowhere"] {
                assert!(!message.contains(value), "{message}");
            }
        }
    }

    /// `Overrides` is printable: a password is `<redacted>`.
    #[test]
    fn the_overrides_do_not_show_a_password() {
        let shown = format!("{:?}", overrides(Some("S3CR3T@127.0.0.1:9001"), None));
        assert!(!shown.contains("S3CR3T"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
    }

    fn refuse(why: &str, say: bool) -> Ask {
        Ask::Refuse {
            why: why.to_string(),
            say,
        }
    }

    /// A reason `strict` gives is a refusal, whatever the other two say: the
    /// update has failed. The log says so when the engine accepted a moment
    /// ago, and not when it refuses already, for the same reason or another.
    #[test]
    fn a_failed_update_asks_for_a_refusal() {
        for due in [true, false] {
            assert_eq!(
                what_the_sources_ask(Some("http status 500"), due, None),
                refuse("http status 500", true),
                "due: {due}"
            );
            assert_eq!(
                what_the_sources_ask(Some("http status 500"), due, Some("http status 500")),
                Ask::Nothing,
                "due: {due}"
            );
            assert_eq!(
                what_the_sources_ask(
                    Some("http status 500"),
                    due,
                    Some("the update has not finished")
                ),
                refuse("http status 500", false),
                "due: {due}"
            );
        }
    }

    /// An update that is due and has not failed is under way: it has not
    /// succeeded, and so nothing is released; and it has not failed, so
    /// nothing is refused. The engine is left as it is.
    #[test]
    fn an_update_that_is_under_way_leaves_the_engine_as_it_is() {
        assert_eq!(what_the_sources_ask(None, true, None), Ask::Nothing);
        assert_eq!(
            what_the_sources_ask(None, true, Some("http status 500")),
            Ask::Nothing
        );
    }

    /// An update that is not due is either up to date or not asked for: a
    /// refusal ends, once, and an engine that accepts is left alone.
    #[test]
    fn an_update_that_is_not_due_releases_a_refusal() {
        assert_eq!(
            what_the_sources_ask(None, false, Some("http status 500")),
            Ask::Accept
        );
        assert_eq!(what_the_sources_ask(None, false, None), Ask::Nothing);
    }

    /// The engine comes up refusing when the sources say the update failed,
    /// and when it is due and has not finished: with the reason the wait at
    /// the start ended with, and without one with a sentence of its own. An
    /// update that is not due any more is no reason, whatever the wait said.
    #[test]
    fn the_engine_starts_refusing_when_the_update_is_owed() {
        let owed = Some("the update has not finished".to_string());
        assert_eq!(
            suspended_at_start(Some("offline"), true, Some("http status 500")),
            Some("offline".to_string())
        );
        assert_eq!(
            suspended_at_start(Some("offline"), false, None),
            Some("offline".to_string())
        );
        assert_eq!(
            suspended_at_start(None, true, Some("http status 500")),
            Some("http status 500".to_string())
        );
        assert_eq!(suspended_at_start(None, true, None), owed);
        assert_eq!(
            suspended_at_start(None, false, Some("http status 500")),
            None
        );
        assert_eq!(suspended_at_start(None, false, None), None);
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
