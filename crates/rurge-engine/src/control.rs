//! Runtime control surface (M4 design §4, §6): the outbound mode as the API
//! and CLI see it, and (Task 3) the `Control` trait the daemon implements.

use rurge_net::BoxFuture;
use rurge_rules::OutboundMode;

/// The outbound mode as exposed by `GET/POST /v1/outbound`. `Proxy` routes
/// everything through the global policy (`Engine::global_policy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Direct,
    Proxy,
    Rule,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Direct => "direct",
            Mode::Proxy => "proxy",
            Mode::Rule => "rule",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "direct" => Some(Mode::Direct),
            "proxy" => Some(Mode::Proxy),
            "rule" => Some(Mode::Rule),
            _ => None,
        }
    }

    /// The CLI's `--outbound-mode` (`proxy=<name>` carries the global policy).
    pub fn from_outbound(mode: &OutboundMode) -> (Mode, Option<String>) {
        match mode {
            OutboundMode::Direct => (Mode::Direct, None),
            OutboundMode::Rule => (Mode::Rule, None),
            OutboundMode::Proxy(p) => (Mode::Proxy, Some(p.name())),
        }
    }
}

/// Outcome of `POST /v1/profiles/reload` / `rurge reload` (M4 design §6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadReport {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub listeners_rebound: bool,
}

/// `POST /v1/log/level` values (phase 1 design §12 mapping is the daemon's job).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Verbose,
    Debug,
    Info,
    Notify,
    Warning,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Verbose => "verbose",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Notify => "notify",
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<LogLevel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "verbose" => Some(LogLevel::Verbose),
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "notify" => Some(LogLevel::Notify),
            "warning" => Some(LogLevel::Warning),
            "error" => Some(LogLevel::Error),
            _ => None,
        }
    }
}

/// What the daemon lets the API drive (M4 design §6). Implemented by the
/// `rurge run` main loop; tests use a fake.
pub trait Control: Send + Sync {
    fn reload(&self) -> BoxFuture<'_, ReloadReport>;
    fn stop(&self) -> BoxFuture<'_, ()>;
    fn set_log_level(&self, level: LogLevel) -> Result<(), String>;
    /// Points the operating system's proxy settings at rurge, or puts them
    /// back. The error text is shown to the API caller.
    fn set_system_proxy(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>>;
    fn system_proxy_enabled(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::rule::PolicyRef;
    use rurge_rules::OutboundMode;

    #[test]
    fn mode_round_trips_and_maps_from_outbound_mode() {
        for m in [Mode::Direct, Mode::Proxy, Mode::Rule] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(Mode::parse("PROXY"), Some(Mode::Proxy));
        assert_eq!(Mode::parse("global"), None);
        assert_eq!(
            Mode::from_outbound(&OutboundMode::Direct),
            (Mode::Direct, None)
        );
        assert_eq!(Mode::from_outbound(&OutboundMode::Rule), (Mode::Rule, None));
        assert_eq!(
            Mode::from_outbound(&OutboundMode::Proxy(PolicyRef::parse("HK"))),
            (Mode::Proxy, Some("HK".to_string()))
        );
    }

    #[test]
    fn log_level_names_round_trip() {
        for (name, level) in [
            ("verbose", LogLevel::Verbose),
            ("debug", LogLevel::Debug),
            ("info", LogLevel::Info),
            ("notify", LogLevel::Notify),
            ("warning", LogLevel::Warning),
            ("error", LogLevel::Error),
        ] {
            assert_eq!(LogLevel::parse(name), Some(level));
            assert_eq!(level.as_str(), name);
        }
        assert_eq!(LogLevel::parse("loud"), None);
    }
}
