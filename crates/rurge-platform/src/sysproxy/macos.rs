//! macOS: `networksetup`, once per enabled network service.

use super::{Backup, ProxySettings, SystemProxy};
use crate::command::{Cmd, CommandRunner, run_all, run_best_effort};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;

const TOOL: &str = "networksetup";

/// The `-get…`, `-set…` and `-set…state` flags of one proxy kind.
struct Kind {
    get: &'static str,
    set: &'static str,
    state: &'static str,
}

const WEB: Kind = Kind {
    get: "-getwebproxy",
    set: "-setwebproxy",
    state: "-setwebproxystate",
};
const SECURE: Kind = Kind {
    get: "-getsecurewebproxy",
    set: "-setsecurewebproxy",
    state: "-setsecurewebproxystate",
};
const SOCKS: Kind = Kind {
    get: "-getsocksfirewallproxy",
    set: "-setsocksfirewallproxy",
    state: "-setsocksfirewallproxystate",
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyState {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceBackup {
    pub web: ProxyState,
    pub secure: ProxyState,
    pub socks: ProxyState,
    pub bypass: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct MacosBackup {
    platform: String,
    services: BTreeMap<String, ServiceBackup>,
}

/// `-listallnetworkservices`: a header line, then one service per line; a
/// leading `*` marks a disabled service.
pub fn parse_services(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("An asterisk") && !l.starts_with('*'))
        .map(str::to_string)
        .collect()
}

/// `Enabled: Yes|No`, `Server: <host>`, `Port: <n>`.
pub fn parse_proxy(output: &str) -> ProxyState {
    let mut state = ProxyState::default();
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Enabled" => state.enabled = value.eq_ignore_ascii_case("yes"),
            "Server" => state.server = value.to_string(),
            "Port" => state.port = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    state
}

/// One domain per line, or a sentence saying there are none.
pub fn parse_bypass(output: &str) -> Vec<String> {
    if output.trim_start().starts_with("There aren't any") {
        return Vec::new();
    }
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn cmd(args: &[&str]) -> Cmd {
    std::iter::once(TOOL)
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect()
}

fn bypass_cmd(service: &str, domains: &[String]) -> Cmd {
    let mut c = cmd(&["-setproxybypassdomains", service]);
    if domains.is_empty() {
        c.push("Empty".to_string());
    } else {
        c.extend(domains.iter().cloned());
    }
    c
}

fn apply_kind(out: &mut Vec<Cmd>, service: &str, kind: &Kind, addr: Option<SocketAddr>) {
    match addr {
        Some(addr) => {
            out.push(cmd(&[
                kind.set,
                service,
                &addr.ip().to_string(),
                &addr.port().to_string(),
            ]));
            out.push(cmd(&[kind.state, service, "on"]));
        }
        None => out.push(cmd(&[kind.state, service, "off"])),
    }
}

pub fn apply_commands(services: &[String], settings: &ProxySettings) -> Vec<Cmd> {
    let mut out = Vec::new();
    for service in services {
        apply_kind(&mut out, service, &WEB, settings.http);
        apply_kind(&mut out, service, &SECURE, settings.https);
        apply_kind(&mut out, service, &SOCKS, settings.socks);
        out.push(bypass_cmd(service, &settings.bypass));
    }
    out
}

fn restore_kind(out: &mut Vec<Cmd>, service: &str, kind: &Kind, state: &ProxyState) {
    if state.server.is_empty() {
        // `-set…proxy` refuses an empty server; off is all there is to restore
        out.push(cmd(&[kind.state, service, "off"]));
    } else {
        out.push(cmd(&[
            kind.set,
            service,
            &state.server,
            &state.port.to_string(),
        ]));
        out.push(cmd(&[
            kind.state,
            service,
            if state.enabled { "on" } else { "off" },
        ]));
    }
}

pub fn restore_commands(services: &BTreeMap<String, ServiceBackup>) -> Vec<Cmd> {
    let mut out = Vec::new();
    for (service, saved) in services {
        restore_kind(&mut out, service, &WEB, &saved.web);
        restore_kind(&mut out, service, &SECURE, &saved.secure);
        restore_kind(&mut out, service, &SOCKS, &saved.socks);
        out.push(bypass_cmd(service, &saved.bypass));
    }
    out
}

pub struct MacosProxy<R: CommandRunner> {
    runner: R,
}

impl<R: CommandRunner> MacosProxy<R> {
    pub fn new(runner: R) -> Self {
        MacosProxy { runner }
    }

    fn services(&self) -> io::Result<Vec<String>> {
        Ok(parse_services(
            &self.runner.run(&cmd(&["-listallnetworkservices"]))?,
        ))
    }

    fn state(&self, kind: &Kind, service: &str) -> io::Result<ProxyState> {
        Ok(parse_proxy(&self.runner.run(&cmd(&[kind.get, service]))?))
    }
}

impl<R: CommandRunner> SystemProxy for MacosProxy<R> {
    fn snapshot(&self) -> io::Result<Backup> {
        let mut services = BTreeMap::new();
        for service in self.services()? {
            let saved = ServiceBackup {
                web: self.state(&WEB, &service)?,
                secure: self.state(&SECURE, &service)?,
                socks: self.state(&SOCKS, &service)?,
                bypass: parse_bypass(
                    &self
                        .runner
                        .run(&cmd(&["-getproxybypassdomains", &service]))?,
                ),
            };
            services.insert(service, saved);
        }
        let backup = MacosBackup {
            platform: "macos".to_string(),
            services,
        };
        Ok(Backup(
            serde_json::to_value(backup).expect("backup serializes"),
        ))
    }

    fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
        let services = self.services()?;
        if services.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "networksetup lists no enabled network service",
            ));
        }
        if settings.exclude_simple {
            tracing::warn!("exclude-simple-hostnames cannot be set through networksetup; ignored");
        }
        run_all(&self.runner, &apply_commands(&services, settings))
    }

    /// Best effort across services: every command runs, the first error is
    /// returned.
    fn restore(&self, backup: &Backup) -> io::Result<()> {
        let saved: MacosBackup =
            serde_json::from_value(backup.0.clone()).map_err(|_| super::wrong_platform("macos"))?;
        if saved.platform != "macos" {
            return Err(super::wrong_platform("macos"));
        }
        run_best_effort(&self.runner, &restore_commands(&saved.services))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::FakeRunner;

    const SERVICES: &str = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Thunderbolt Bridge\nUSB 10/100/1000 LAN\n";

    fn settings() -> ProxySettings {
        ProxySettings {
            http: Some("127.0.0.1:6152".parse().unwrap()),
            https: Some("127.0.0.1:6152".parse().unwrap()),
            socks: None,
            bypass: vec!["localhost".into(), "192.168.0.0/16".into()],
            exclude_simple: false,
        }
    }

    #[test]
    fn parses_the_service_list_skipping_the_header_and_disabled_services() {
        assert_eq!(parse_services(SERVICES), ["Wi-Fi", "USB 10/100/1000 LAN"]);
        assert!(parse_services("").is_empty());
    }

    #[test]
    fn parses_proxy_state() {
        let on = parse_proxy(
            "Enabled: Yes\nServer: 10.0.0.1\nPort: 8080\nAuthenticated Proxy Enabled: 0\n",
        );
        assert_eq!(
            on,
            ProxyState {
                enabled: true,
                server: "10.0.0.1".into(),
                port: 8080
            }
        );
        let off = parse_proxy("Enabled: No\nServer: \nPort: 0\nAuthenticated Proxy Enabled: 0\n");
        assert_eq!(off, ProxyState::default());
        assert_eq!(
            parse_proxy("Enabled: Yes\nServer: ::1\nPort: 1\n").server,
            "::1"
        );
    }

    #[test]
    fn parses_bypass_domains() {
        assert_eq!(
            parse_bypass("*.local\n169.254/16\n"),
            ["*.local", "169.254/16"]
        );
        assert!(parse_bypass("There aren't any bypass domains set on Wi-Fi.\n").is_empty());
    }

    #[test]
    fn apply_commands_cover_every_kind_for_every_service() {
        let cmds: Vec<String> = apply_commands(&["Wi-Fi".to_string()], &settings())
            .iter()
            .map(|c| c.join(" "))
            .collect();
        assert_eq!(
            cmds,
            [
                "networksetup -setwebproxy Wi-Fi 127.0.0.1 6152",
                "networksetup -setwebproxystate Wi-Fi on",
                "networksetup -setsecurewebproxy Wi-Fi 127.0.0.1 6152",
                "networksetup -setsecurewebproxystate Wi-Fi on",
                "networksetup -setsocksfirewallproxystate Wi-Fi off",
                "networksetup -setproxybypassdomains Wi-Fi localhost 192.168.0.0/16",
            ]
        );
        let no_bypass = ProxySettings {
            bypass: Vec::new(),
            ..settings()
        };
        let last = apply_commands(&["Wi-Fi".to_string()], &no_bypass)
            .pop()
            .unwrap();
        assert_eq!(
            last.join(" "),
            "networksetup -setproxybypassdomains Wi-Fi Empty"
        );
        // a service name with spaces stays one argument
        let spaced = apply_commands(&["USB 10/100/1000 LAN".to_string()], &settings());
        assert_eq!(spaced[0][2], "USB 10/100/1000 LAN");
    }

    fn runner_with_one_service() -> FakeRunner {
        let runner = FakeRunner::default();
        runner.reply(
            "networksetup -listallnetworkservices",
            "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n",
        );
        runner.reply(
            "networksetup -getwebproxy Wi-Fi",
            "Enabled: Yes\nServer: corp\nPort: 3128\n",
        );
        runner.reply(
            "networksetup -getsecurewebproxy Wi-Fi",
            "Enabled: No\nServer: corp\nPort: 3128\n",
        );
        runner.reply(
            "networksetup -getsocksfirewallproxy Wi-Fi",
            "Enabled: No\nServer: \nPort: 0\n",
        );
        runner.reply("networksetup -getproxybypassdomains Wi-Fi", "*.local\n");
        runner
    }

    #[test]
    fn snapshot_then_restore_puts_every_service_back() {
        let proxy = MacosProxy::new(runner_with_one_service());
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["platform"], "macos");
        assert_eq!(backup.0["services"]["Wi-Fi"]["web"]["server"], "corp");
        assert_eq!(backup.0["services"]["Wi-Fi"]["bypass"][0], "*.local");
        proxy.runner.calls.lock().unwrap().clear();
        proxy.restore(&backup).unwrap();
        assert_eq!(
            proxy.runner.calls(),
            [
                "networksetup -setwebproxy Wi-Fi corp 3128",
                "networksetup -setwebproxystate Wi-Fi on",
                "networksetup -setsecurewebproxy Wi-Fi corp 3128",
                "networksetup -setsecurewebproxystate Wi-Fi off",
                "networksetup -setsocksfirewallproxystate Wi-Fi off",
                "networksetup -setproxybypassdomains Wi-Fi *.local",
            ]
        );
    }

    #[test]
    fn apply_stops_at_the_first_failure_and_reports_it() {
        let runner = runner_with_one_service();
        runner.fail(
            "networksetup -setwebproxy Wi-Fi 127.0.0.1 6152",
            "** Error: Command requires admin privileges.",
        );
        let proxy = MacosProxy::new(runner);
        let err = proxy.apply(&settings()).unwrap_err();
        assert!(
            err.to_string().contains("requires admin privileges"),
            "{err}"
        );
        let calls = proxy.runner.calls();
        assert_eq!(
            calls.last().unwrap(),
            "networksetup -setwebproxy Wi-Fi 127.0.0.1 6152"
        );
    }

    #[test]
    fn restore_keeps_going_after_a_failure_and_returns_the_first_error() {
        let proxy = MacosProxy::new(runner_with_one_service());
        let backup = proxy.snapshot().unwrap();
        proxy
            .runner
            .fail("networksetup -setwebproxy Wi-Fi corp 3128", "boom");
        proxy.runner.calls.lock().unwrap().clear();
        let err = proxy.restore(&backup).unwrap_err();
        assert!(err.to_string().contains("boom"));
        assert_eq!(
            proxy.runner.calls().len(),
            6,
            "the remaining commands still ran"
        );
    }

    #[test]
    fn apply_needs_an_enabled_service_and_restore_rejects_foreign_backups() {
        let runner = FakeRunner::default();
        runner.reply(
            "networksetup -listallnetworkservices",
            "An asterisk (*) denotes that a network service is disabled.\n*Wi-Fi\n",
        );
        let proxy = MacosProxy::new(runner);
        assert_eq!(
            proxy.apply(&settings()).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let foreign = Backup(serde_json::json!({ "platform": "windows" }));
        assert_eq!(
            proxy.restore(&foreign).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
