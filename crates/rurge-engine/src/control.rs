//! Runtime control surface (M4 design §4, §6): the outbound mode as the API
//! and CLI see it, and (Task 3) the `Control` trait the daemon implements.

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
}
