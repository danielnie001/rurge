//! Linux: GNOME through `gsettings`, KDE through `kwriteconfig`; any other
//! desktop gets an environment-variable hint and nothing is changed.

use super::{Backup, ProxySettings, SystemProxy};
use crate::command::{Cmd, CommandRunner, run_all, run_best_effort};
use crate::dirs::EnvLookup;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, SocketAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Gnome,
    Kde,
    Other,
}

const GNOME_ROOT: &str = "org.gnome.system.proxy";
/// (schema, key) pairs rurge touches, in snapshot order.
const GNOME_KEYS: [(&str, &str); 8] = [
    ("org.gnome.system.proxy", "mode"),
    ("org.gnome.system.proxy", "ignore-hosts"),
    ("org.gnome.system.proxy.http", "host"),
    ("org.gnome.system.proxy.http", "port"),
    ("org.gnome.system.proxy.https", "host"),
    ("org.gnome.system.proxy.https", "port"),
    ("org.gnome.system.proxy.socks", "host"),
    ("org.gnome.system.proxy.socks", "port"),
];
const KDE_FILE: &str = "kioslaverc";
const KDE_GROUP: &str = "Proxy Settings";
const KDE_KEYS: [&str; 5] = [
    "ProxyType",
    "httpProxy",
    "httpsProxy",
    "socksProxy",
    "NoProxyFor",
];

/// GNOME needs `gsettings` and `GNOME` in `XDG_CURRENT_DESKTOP`; KDE needs
/// `KDE` there and a `kwriteconfig` / `kreadconfig` pair.
pub fn detect(env: EnvLookup<'_>, has_tool: &dyn Fn(&str) -> bool) -> Desktop {
    let desktop = env("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_default();
    if desktop.contains("GNOME") && has_tool("gsettings") {
        Desktop::Gnome
    } else if desktop.contains("KDE") && kde_tools(has_tool).is_some() {
        Desktop::Kde
    } else {
        Desktop::Other
    }
}

/// (write tool, read tool): Plasma 6 first, then Plasma 5.
pub fn kde_tools(has_tool: &dyn Fn(&str) -> bool) -> Option<(String, String)> {
    [
        ("kwriteconfig6", "kreadconfig6"),
        ("kwriteconfig5", "kreadconfig5"),
    ]
    .into_iter()
    .find(|(write, read)| has_tool(write) && has_tool(read))
    .map(|(write, read)| (write.to_string(), read.to_string()))
}

pub fn tool_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

fn gsettings(args: &[&str]) -> Cmd {
    std::iter::once("gsettings")
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect()
}

/// A GVariant string literal.
fn gv_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// A GVariant string array; the empty one needs its type spelled out.
fn gv_strv(items: &[String]) -> String {
    if items.is_empty() {
        return "@as []".to_string();
    }
    let quoted: Vec<String> = items.iter().map(|s| gv_str(s)).collect();
    format!("[{}]", quoted.join(", "))
}

pub fn gnome_apply_commands(settings: &ProxySettings) -> Vec<Cmd> {
    let mut out = Vec::new();
    for (kind, addr) in [
        ("http", settings.http),
        ("https", settings.https),
        ("socks", settings.socks),
    ] {
        let schema = format!("{GNOME_ROOT}.{kind}");
        let (host, port) = match addr {
            Some(addr) => (addr.ip().to_string(), addr.port()),
            None => (String::new(), 0),
        };
        out.push(gsettings(&["set", &schema, "host", &gv_str(&host)]));
        out.push(gsettings(&["set", &schema, "port", &port.to_string()]));
    }
    out.push(gsettings(&[
        "set",
        GNOME_ROOT,
        "ignore-hosts",
        &gv_strv(&settings.bypass),
    ]));
    // switched to manual last, once the addresses are in place
    out.push(gsettings(&["set", GNOME_ROOT, "mode", "'manual'"]));
    out
}

fn kwrite(tool: &str, key: &str, value: Option<&str>) -> Cmd {
    let mut c: Cmd = [tool, "--file", KDE_FILE, "--group", KDE_GROUP, "--key", key]
        .iter()
        .map(|s| s.to_string())
        .collect();
    c.push(value.unwrap_or("--delete").to_string());
    c
}

fn kread(tool: &str, key: &str) -> Cmd {
    [tool, "--file", KDE_FILE, "--group", KDE_GROUP, "--key", key]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// `kioslaverc` keeps a proxy as `<scheme>://<host> <port>`.
fn kde_url(scheme: &str, addr: SocketAddr) -> String {
    let host = match addr.ip() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    format!("{scheme}://{host} {}", addr.port())
}

pub fn kde_apply_commands(write_tool: &str, settings: &ProxySettings) -> Vec<Cmd> {
    let http = settings.http.map(|a| kde_url("http", a));
    let https = settings.https.map(|a| kde_url("http", a));
    let socks = settings.socks.map(|a| kde_url("socks", a));
    let bypass = (!settings.bypass.is_empty()).then(|| settings.bypass.join(","));
    vec![
        kwrite(write_tool, "httpProxy", http.as_deref()),
        kwrite(write_tool, "httpsProxy", https.as_deref()),
        kwrite(write_tool, "socksProxy", socks.as_deref()),
        kwrite(write_tool, "NoProxyFor", bypass.as_deref()),
        kwrite(write_tool, "ProxyType", Some("1")),
    ]
}

/// Asks running KIO clients to re-read `kioslaverc`.
fn kio_reparse_cmd() -> Cmd {
    [
        "dbus-send",
        "--type=signal",
        "/KIO/Scheduler",
        "org.kde.KIO.Scheduler.reparseSlaveConfiguration",
        "string:",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// What a shell user can export when no desktop setting exists.
pub fn env_hint(settings: &ProxySettings) -> String {
    let mut vars = Vec::new();
    if let Some(addr) = settings.http {
        vars.push(format!("http_proxy=http://{addr}"));
    }
    if let Some(addr) = settings.https {
        vars.push(format!("https_proxy=http://{addr}"));
    }
    if !settings.bypass.is_empty() {
        vars.push(format!("no_proxy={}", settings.bypass.join(",")));
    }
    format!("export {}", vars.join(" "))
}

#[derive(Serialize, Deserialize)]
struct LinuxBackup {
    platform: String,
    desktop: String,
    #[serde(default)]
    values: BTreeMap<String, String>,
}

pub struct LinuxProxy<R: CommandRunner> {
    runner: R,
    desktop: Desktop,
    kde: Option<(String, String)>,
}

impl<R: CommandRunner> LinuxProxy<R> {
    pub fn new(runner: R, desktop: Desktop, kde: Option<(String, String)>) -> Self {
        LinuxProxy {
            runner,
            desktop,
            kde,
        }
    }

    /// Looks at this process's environment and `PATH`.
    pub fn detect(runner: R) -> Self {
        let has_tool = |name: &str| tool_on_path(name);
        LinuxProxy::new(
            runner,
            detect(&|key| std::env::var_os(key), &has_tool),
            kde_tools(&has_tool),
        )
    }

    fn kde(&self) -> io::Result<&(String, String)> {
        self.kde.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "kwriteconfig6 / kwriteconfig5 not found",
            )
        })
    }

    fn notify_kio(&self) {
        if let Err(e) = self.runner.run(&kio_reparse_cmd()) {
            tracing::debug!(error = %e, "cannot tell KIO to re-read its proxy settings");
        }
    }
}

impl<R: CommandRunner> SystemProxy for LinuxProxy<R> {
    fn snapshot(&self) -> io::Result<Backup> {
        let mut values = BTreeMap::new();
        let desktop = match self.desktop {
            Desktop::Gnome => {
                for (schema, key) in GNOME_KEYS {
                    let out = self.runner.run(&gsettings(&["get", schema, key]))?;
                    values.insert(format!("{schema} {key}"), out.trim().to_string());
                }
                "gnome"
            }
            Desktop::Kde => {
                let (_, read) = self.kde()?;
                for key in KDE_KEYS {
                    let out = self.runner.run(&kread(read, key))?;
                    values.insert(key.to_string(), out.trim().to_string());
                }
                "kde"
            }
            Desktop::Other => "none",
        };
        let backup = LinuxBackup {
            platform: "linux".to_string(),
            desktop: desktop.to_string(),
            values,
        };
        Ok(Backup(
            serde_json::to_value(backup).expect("backup serializes"),
        ))
    }

    fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
        match self.desktop {
            Desktop::Gnome => run_all(&self.runner, &gnome_apply_commands(settings)),
            Desktop::Kde => {
                let (write, _) = self.kde()?;
                run_all(&self.runner, &kde_apply_commands(write, settings))?;
                self.notify_kio();
                Ok(())
            }
            Desktop::Other => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "no supported desktop proxy settings (GNOME or KDE) were found; set the proxy in your shell instead:\n  {}",
                    env_hint(settings)
                ),
            )),
        }
    }

    /// Follows the desktop recorded in the backup, not the current session.
    fn restore(&self, backup: &Backup) -> io::Result<()> {
        let saved: LinuxBackup =
            serde_json::from_value(backup.0.clone()).map_err(|_| super::wrong_platform("linux"))?;
        if saved.platform != "linux" {
            return Err(super::wrong_platform("linux"));
        }
        match saved.desktop.as_str() {
            "gnome" => {
                // everything but the mode first, the mode last
                let ordered = GNOME_KEYS
                    .iter()
                    .filter(|(_, key)| *key != "mode")
                    .chain(GNOME_KEYS.iter().filter(|(_, key)| *key == "mode"));
                let cmds: Vec<Cmd> = ordered
                    .filter_map(|&(schema, key)| {
                        let value = saved.values.get(&format!("{schema} {key}"))?;
                        (!value.is_empty())
                            .then(|| gsettings(&["set", schema, key, value.as_str()]))
                    })
                    .collect();
                run_best_effort(&self.runner, &cmds)
            }
            "kde" => {
                let (write, _) = self.kde()?;
                let cmds: Vec<Cmd> = KDE_KEYS
                    .iter()
                    .filter_map(|key| {
                        let value = saved.values.get(*key)?;
                        Some(kwrite(
                            write,
                            key,
                            (!value.is_empty()).then_some(value.as_str()),
                        ))
                    })
                    .collect();
                let result = run_best_effort(&self.runner, &cmds);
                self.notify_kio();
                result
            }
            "none" => Ok(()),
            _ => Err(super::wrong_platform("linux")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::FakeRunner;
    use std::ffi::OsString;

    fn settings() -> ProxySettings {
        ProxySettings {
            http: Some("127.0.0.1:6152".parse().unwrap()),
            https: Some("127.0.0.1:6152".parse().unwrap()),
            socks: Some("127.0.0.1:6153".parse().unwrap()),
            bypass: vec!["localhost".into(), "192.168.0.0/16".into()],
            exclude_simple: false,
        }
    }

    fn desktop_env(value: &'static str) -> impl Fn(&str) -> Option<OsString> {
        move |key| (key == "XDG_CURRENT_DESKTOP").then(|| OsString::from(value))
    }

    fn lines(cmds: &[Cmd]) -> Vec<String> {
        cmds.iter().map(|c| c.join(" ")).collect()
    }

    #[test]
    fn detects_the_desktop_from_the_environment_and_the_tools() {
        let all = |_: &str| true;
        let none = |_: &str| false;
        let plasma5 = |name: &str| name.ends_with('5');
        assert_eq!(detect(&desktop_env("ubuntu:GNOME"), &all), Desktop::Gnome);
        assert_eq!(
            detect(&desktop_env("GNOME"), &none),
            Desktop::Other,
            "gsettings is required"
        );
        assert_eq!(detect(&desktop_env("KDE"), &all), Desktop::Kde);
        assert_eq!(
            detect(&desktop_env("KDE"), &plasma5),
            Desktop::Kde,
            "Plasma 5 tools are accepted"
        );
        assert_eq!(detect(&desktop_env("KDE"), &none), Desktop::Other);
        assert_eq!(detect(&desktop_env("XFCE"), &all), Desktop::Other);
        assert_eq!(detect(&|_| None, &all), Desktop::Other);
        assert_eq!(
            kde_tools(&all),
            Some(("kwriteconfig6".to_string(), "kreadconfig6".to_string()))
        );
        assert_eq!(
            kde_tools(&plasma5),
            Some(("kwriteconfig5".to_string(), "kreadconfig5".to_string()))
        );
        assert_eq!(kde_tools(&none), None);
    }

    #[test]
    fn gnome_apply_sets_hosts_and_bypass_then_switches_to_manual() {
        assert_eq!(
            lines(&gnome_apply_commands(&settings())),
            [
                "gsettings set org.gnome.system.proxy.http host '127.0.0.1'",
                "gsettings set org.gnome.system.proxy.http port 6152",
                "gsettings set org.gnome.system.proxy.https host '127.0.0.1'",
                "gsettings set org.gnome.system.proxy.https port 6152",
                "gsettings set org.gnome.system.proxy.socks host '127.0.0.1'",
                "gsettings set org.gnome.system.proxy.socks port 6153",
                "gsettings set org.gnome.system.proxy ignore-hosts ['localhost', '192.168.0.0/16']",
                "gsettings set org.gnome.system.proxy mode 'manual'",
            ]
        );
        let bare = ProxySettings {
            socks: None,
            bypass: Vec::new(),
            ..settings()
        };
        let cmds = lines(&gnome_apply_commands(&bare));
        assert!(
            cmds.contains(&"gsettings set org.gnome.system.proxy.socks host ''".to_string()),
            "{cmds:?}"
        );
        assert!(cmds.contains(&"gsettings set org.gnome.system.proxy.socks port 0".to_string()));
        assert!(
            cmds.contains(&"gsettings set org.gnome.system.proxy ignore-hosts @as []".to_string())
        );
        // the GVariant text is one argument, quotes escaped
        let quoted = ProxySettings {
            bypass: vec!["it's".into()],
            ..settings()
        };
        let ignore = &gnome_apply_commands(&quoted)[6];
        assert_eq!(ignore[4], r"['it\'s']");
    }

    fn gnome_runner() -> FakeRunner {
        let runner = FakeRunner::default();
        for (key, value) in [
            ("org.gnome.system.proxy mode", "'none'"),
            (
                "org.gnome.system.proxy ignore-hosts",
                "['localhost', '127.0.0.0/8', '::1']",
            ),
            ("org.gnome.system.proxy.http host", "''"),
            ("org.gnome.system.proxy.http port", "8080"),
            ("org.gnome.system.proxy.https host", "''"),
            ("org.gnome.system.proxy.https port", "0"),
            ("org.gnome.system.proxy.socks host", "''"),
            ("org.gnome.system.proxy.socks port", "0"),
        ] {
            runner.reply(&format!("gsettings get {key}"), &format!("{value}\n"));
        }
        runner
    }

    #[test]
    fn gnome_snapshot_then_restore_writes_the_raw_values_back_with_mode_last() {
        let proxy = LinuxProxy::new(gnome_runner(), Desktop::Gnome, None);
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["platform"], "linux");
        assert_eq!(backup.0["desktop"], "gnome");
        assert_eq!(
            backup.0["values"]["org.gnome.system.proxy.http port"],
            "8080"
        );
        proxy.apply(&settings()).unwrap();
        proxy.runner.calls.lock().unwrap().clear();
        proxy.restore(&backup).unwrap();
        let calls = proxy.runner.calls();
        assert_eq!(calls.len(), 8);
        assert_eq!(
            calls[0],
            "gsettings set org.gnome.system.proxy ignore-hosts ['localhost', '127.0.0.0/8', '::1']"
        );
        assert_eq!(
            calls[2],
            "gsettings set org.gnome.system.proxy.http port 8080"
        );
        assert_eq!(
            calls[7], "gsettings set org.gnome.system.proxy mode 'none'",
            "the mode goes back last"
        );
    }

    #[test]
    fn kde_apply_writes_kioslaverc_and_tells_kio() {
        let cmds = kde_apply_commands("kwriteconfig6", &settings());
        assert_eq!(cmds[0][4], "Proxy Settings", "the group is one argument");
        assert_eq!(
            lines(&cmds),
            [
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key httpProxy http://127.0.0.1 6152",
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key httpsProxy http://127.0.0.1 6152",
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key socksProxy socks://127.0.0.1 6153",
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key NoProxyFor localhost,192.168.0.0/16",
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key ProxyType 1",
            ]
        );
        let bare = ProxySettings {
            socks: None,
            bypass: Vec::new(),
            ..settings()
        };
        let cmds = lines(&kde_apply_commands("kwriteconfig5", &bare));
        assert_eq!(
            cmds[2],
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key socksProxy --delete"
        );
        assert_eq!(
            cmds[3],
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key NoProxyFor --delete"
        );
        let v6 = ProxySettings {
            http: Some("[::1]:6152".parse().unwrap()),
            ..settings()
        };
        assert_eq!(
            kde_apply_commands("kwriteconfig6", &v6)[0][7],
            "http://[::1] 6152"
        );
    }

    #[test]
    fn kde_snapshot_then_restore_deletes_what_was_unset() {
        let runner = FakeRunner::default();
        runner.reply(
            "kreadconfig6 --file kioslaverc --group Proxy Settings --key ProxyType",
            "0\n",
        );
        runner.reply(
            "kreadconfig6 --file kioslaverc --group Proxy Settings --key NoProxyFor",
            "localhost\n",
        );
        let tools = Some(("kwriteconfig6".to_string(), "kreadconfig6".to_string()));
        let proxy = LinuxProxy::new(runner, Desktop::Kde, tools);
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["desktop"], "kde");
        assert_eq!(backup.0["values"]["ProxyType"], "0");
        assert_eq!(backup.0["values"]["httpProxy"], "");
        proxy.apply(&settings()).unwrap();
        assert!(
            proxy
                .runner
                .calls()
                .last()
                .unwrap()
                .starts_with("dbus-send "),
            "KIO is told to re-read its configuration"
        );
        proxy.runner.calls.lock().unwrap().clear();
        proxy.restore(&backup).unwrap();
        let calls = proxy.runner.calls();
        assert!(
            calls.contains(
                &"kwriteconfig6 --file kioslaverc --group Proxy Settings --key ProxyType 0"
                    .to_string()
            ),
            "{calls:?}"
        );
        assert!(
            calls.contains(
                &"kwriteconfig6 --file kioslaverc --group Proxy Settings --key httpProxy --delete"
                    .to_string()
            )
        );
        assert!(calls.contains(&"kwriteconfig6 --file kioslaverc --group Proxy Settings --key NoProxyFor localhost".to_string()));
        assert!(calls.last().unwrap().starts_with("dbus-send "));
    }

    #[test]
    fn a_failing_dbus_notification_does_not_fail_the_apply() {
        let runner = FakeRunner::default();
        runner.fail(&kio_reparse_cmd().join(" "), "no session bus");
        let tools = Some(("kwriteconfig6".to_string(), "kreadconfig6".to_string()));
        let proxy = LinuxProxy::new(runner, Desktop::Kde, tools);
        proxy.apply(&settings()).unwrap();
    }

    #[test]
    fn other_desktops_get_an_environment_hint_and_change_nothing() {
        let proxy = LinuxProxy::new(FakeRunner::default(), Desktop::Other, None);
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["desktop"], "none");
        let err = proxy.apply(&settings()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        let text = err.to_string();
        assert!(
            text.contains("export http_proxy=http://127.0.0.1:6152 https_proxy=http://127.0.0.1:6152 no_proxy=localhost,192.168.0.0/16"),
            "{text}"
        );
        proxy.restore(&backup).unwrap();
        assert!(proxy.runner.calls().is_empty(), "nothing was run");
    }

    #[test]
    fn restore_follows_the_backup_not_the_current_desktop() {
        // the backup was taken under GNOME; the session is something else now
        let gnome = LinuxProxy::new(gnome_runner(), Desktop::Gnome, None);
        let backup = gnome.snapshot().unwrap();
        let other = LinuxProxy::new(FakeRunner::default(), Desktop::Other, None);
        other.restore(&backup).unwrap();
        assert_eq!(other.runner.calls().len(), 8);
        let foreign = Backup(serde_json::json!({ "platform": "windows" }));
        assert_eq!(
            other.restore(&foreign).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
