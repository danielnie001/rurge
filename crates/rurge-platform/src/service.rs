//! `rurge service install | uninstall` (M4 design §8.3): the unit / plist /
//! logon task that starts rurge, and the commands that register it. Plans
//! take an explicit `Os`, so every platform is tested on every host; only
//! `execute` touches the system.

use crate::command::{Cmd, CommandRunner};
use crate::dirs::{EnvLookup, Os, home};
use std::io;
use std::path::{Path, PathBuf};

pub const SYSTEMD_UNIT: &str = "rurge.service";
pub const LAUNCHD_LABEL: &str = "io.rurge.daemon";
pub const TASK_NAME: &str = "rurge";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    System,
}

#[derive(Clone, Debug)]
pub struct ServiceSpec {
    /// Absolute path of the rurge binary.
    pub exe: PathBuf,
    /// Absolute path of the profile.
    pub config: PathBuf,
    pub system_proxy: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub files: Vec<(PathBuf, String)>,
    pub commands: Vec<Cmd>,
    pub remove: Vec<PathBuf>,
}

fn cmd(args: &[&str]) -> Cmd {
    args.iter().map(|s| s.to_string()).collect()
}

fn systemd_unit_path(scope: Scope, env: EnvLookup<'_>) -> PathBuf {
    match scope {
        Scope::User => home(env)
            .join(".config")
            .join("systemd")
            .join("user")
            .join(SYSTEMD_UNIT),
        Scope::System => PathBuf::from("/etc/systemd/system").join(SYSTEMD_UNIT),
    }
}

fn systemctl(scope: Scope, verb: &str) -> Cmd {
    match scope {
        Scope::User => cmd(&["systemctl", "--user", verb, "--now", "rurge"]),
        Scope::System => cmd(&["systemctl", verb, "--now", "rurge"]),
    }
}

/// Double-quoted for `ExecStart=`: `\` and `"` escaped, `%` and `$` doubled
/// (systemd expands specifiers and variables there).
fn systemd_quote(path: &Path) -> String {
    let escaped = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    format!("\"{escaped}\"")
}

fn systemd_unit(spec: &ServiceSpec, scope: Scope) -> String {
    let mut exec = format!(
        "{} run -c {}",
        systemd_quote(&spec.exe),
        systemd_quote(&spec.config)
    );
    if spec.system_proxy {
        exec.push_str(" --system-proxy");
    }
    // `--system-proxy` needs a desktop session (GNOME / KDE settings, the
    // session bus), so that unit is tied to the graphical session rather than
    // to login: it starts once `XDG_CURRENT_DESKTOP` and the bus are there,
    // and stops with the session.
    let graphical = scope == Scope::User && spec.system_proxy;
    let session = if graphical {
        "PartOf=graphical-session.target\nAfter=graphical-session.target\n"
    } else {
        ""
    };
    let target = match (graphical, scope) {
        (true, _) => "graphical-session.target",
        (false, Scope::User) => "default.target",
        (false, Scope::System) => "multi-user.target",
    };
    // a start limit, so a unit that can never work stops restarting every 3 s
    format!(
        "[Unit]\nDescription=rurge proxy\nAfter=network-online.target\nWants=network-online.target\n\
{session}StartLimitIntervalSec=60\nStartLimitBurst=5\n\n\
[Service]\nExecStart={exec}\nRestart=on-failure\nRestartSec=3\n\n\
[Install]\nWantedBy={target}\n"
    )
}

fn launchd_plist_path(scope: Scope, env: EnvLookup<'_>) -> PathBuf {
    let file = format!("{LAUNCHD_LABEL}.plist");
    match scope {
        Scope::User => home(env).join("Library").join("LaunchAgents").join(file),
        Scope::System => PathBuf::from("/Library/LaunchDaemons").join(file),
    }
}

fn launchd_domain(scope: Scope, uid: Option<u32>) -> io::Result<String> {
    match (scope, uid) {
        (Scope::System, _) => Ok("system".to_string()),
        (Scope::User, Some(uid)) => Ok(format!("gui/{uid}")),
        (Scope::User, None) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot determine the user id for the launchd domain",
        )),
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn launchd_plist(spec: &ServiceSpec) -> String {
    let mut args = vec![
        spec.exe.display().to_string(),
        "run".to_string(),
        "-c".to_string(),
        spec.config.display().to_string(),
    ];
    if spec.system_proxy {
        args.push("--system-proxy".to_string());
    }
    let args: String = args
        .iter()
        .map(|a| format!("        <string>{}</string>\n", xml_escape(a)))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>{LAUNCHD_LABEL}</string>\n    <key>ProgramArguments</key>\n    <array>\n{args}    </array>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>\n</dict>\n</plist>\n"
    )
}

/// The command line the logon task runs.
fn task_command(spec: &ServiceSpec) -> String {
    let mut line = format!(
        "\"{}\" run -c \"{}\"",
        spec.exe.display(),
        spec.config.display()
    );
    if spec.system_proxy {
        line.push_str(" --system-proxy");
    }
    line
}

pub fn install_plan(
    os: Os,
    scope: Scope,
    spec: &ServiceSpec,
    env: EnvLookup<'_>,
    uid: Option<u32>,
) -> io::Result<Plan> {
    Ok(match os {
        Os::Unix => {
            // A system unit runs as root with no desktop session, so the Linux
            // backend would fail with `Unsupported` on every start.
            if scope == Scope::System && spec.system_proxy {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--system-proxy needs a desktop session: on Linux install with --user",
                ));
            }
            Plan {
                files: vec![(systemd_unit_path(scope, env), systemd_unit(spec, scope))],
                commands: vec![systemctl(scope, "enable")],
                remove: Vec::new(),
            }
        }
        Os::MacOs => {
            let plist = launchd_plist_path(scope, env);
            let domain = launchd_domain(scope, uid)?;
            Plan {
                commands: vec![cmd(&[
                    "launchctl",
                    "bootstrap",
                    &domain,
                    &plist.display().to_string(),
                ])],
                files: vec![(plist, launchd_plist(spec))],
                remove: Vec::new(),
            }
        }
        // `/f`: without it schtasks asks before replacing an existing task
        Os::Windows => Plan {
            files: Vec::new(),
            commands: vec![cmd(&[
                "schtasks",
                "/create",
                "/tn",
                TASK_NAME,
                "/sc",
                "onlogon",
                "/tr",
                &task_command(spec),
                "/f",
            ])],
            remove: Vec::new(),
        },
    })
}

pub fn uninstall_plan(
    os: Os,
    scope: Scope,
    env: EnvLookup<'_>,
    uid: Option<u32>,
) -> io::Result<Plan> {
    Ok(match os {
        Os::Unix => Plan {
            files: Vec::new(),
            commands: vec![systemctl(scope, "disable")],
            remove: vec![systemd_unit_path(scope, env)],
        },
        Os::MacOs => {
            let plist = launchd_plist_path(scope, env);
            let domain = launchd_domain(scope, uid)?;
            Plan {
                files: Vec::new(),
                commands: vec![cmd(&[
                    "launchctl",
                    "bootout",
                    &domain,
                    &plist.display().to_string(),
                ])],
                remove: vec![plist],
            }
        }
        Os::Windows => Plan {
            files: Vec::new(),
            commands: vec![cmd(&["schtasks", "/delete", "/tn", TASK_NAME, "/f"])],
            remove: Vec::new(),
        },
    })
}

/// Writes the files, runs the commands, removes what is to be removed. An
/// install stops at the first failing command; an uninstall carries on so its
/// files still go away, and returns the first error.
pub fn execute(plan: &Plan, runner: &dyn CommandRunner) -> io::Result<()> {
    for (path, content) in &plan.files {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("cannot create directory {}: {e}", dir.display()),
                )
            })?;
        }
        std::fs::write(path, content).map_err(|e| {
            io::Error::new(e.kind(), format!("cannot write {}: {e}", path.display()))
        })?;
    }
    let mut first_error = None;
    for command in &plan.commands {
        if let Err(e) = runner.run(command) {
            if plan.remove.is_empty() {
                return Err(e);
            }
            first_error.get_or_insert(e);
        }
    }
    for path in &plan.remove {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                first_error.get_or_insert(io::Error::new(
                    e.kind(),
                    format!("cannot remove {}: {e}", path.display()),
                ));
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::FakeRunner;
    use std::ffi::OsString;

    fn spec(system_proxy: bool) -> ServiceSpec {
        ServiceSpec {
            exe: PathBuf::from("/usr/local/bin/rurge"),
            config: PathBuf::from("/home/me/my profiles/rurge.conf"),
            system_proxy,
        }
    }

    fn home_env(key: &str) -> Option<OsString> {
        (key == "HOME").then(|| OsString::from("/home/me"))
    }

    fn lines(cmds: &[Cmd]) -> Vec<String> {
        cmds.iter().map(|c| c.join(" ")).collect()
    }

    #[test]
    fn systemd_user_unit() {
        let plan = install_plan(Os::Unix, Scope::User, &spec(true), &home_env, None).unwrap();
        let unit = PathBuf::from("/home/me")
            .join(".config")
            .join("systemd")
            .join("user")
            .join(SYSTEMD_UNIT);
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].0, unit);
        let text = &plan.files[0].1;
        assert!(
            text.contains("ExecStart=\"/usr/local/bin/rurge\" run -c \"/home/me/my profiles/rurge.conf\" --system-proxy\n"),
            "{text}"
        );
        // `spec(true)` carries `--system-proxy`, so this unit follows the
        // graphical session; the plain user unit is covered further down.
        assert!(
            text.contains("Restart=on-failure\n")
                && text.contains("WantedBy=graphical-session.target\n"),
            "{text}"
        );
        assert_eq!(
            lines(&plan.commands),
            ["systemctl --user enable --now rurge"]
        );
        assert!(plan.remove.is_empty());
        let removal = uninstall_plan(Os::Unix, Scope::User, &home_env, None).unwrap();
        assert_eq!(
            lines(&removal.commands),
            ["systemctl --user disable --now rurge"]
        );
        assert_eq!(removal.remove, [unit]);
        assert!(removal.files.is_empty());
    }

    #[test]
    fn systemd_system_unit() {
        let plan = install_plan(Os::Unix, Scope::System, &spec(false), &home_env, None).unwrap();
        assert_eq!(
            plan.files[0].0,
            PathBuf::from("/etc/systemd/system").join(SYSTEMD_UNIT)
        );
        let text = &plan.files[0].1;
        assert!(text.contains("WantedBy=multi-user.target\n"), "{text}");
        assert!(!text.contains("--system-proxy"));
        assert_eq!(lines(&plan.commands), ["systemctl enable --now rurge"]);
    }

    #[test]
    fn a_linux_system_unit_cannot_carry_system_proxy() {
        let err = install_plan(Os::Unix, Scope::System, &spec(true), &home_env, None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "--system-proxy needs a desktop session: on Linux install with --user"
        );
        // the other two platforms have no such restriction
        assert!(install_plan(Os::MacOs, Scope::System, &spec(true), &home_env, None).is_ok());
        assert!(install_plan(Os::Windows, Scope::System, &spec(true), &home_env, None).is_ok());
    }

    #[test]
    fn a_user_unit_with_system_proxy_follows_the_graphical_session() {
        let with = install_plan(Os::Unix, Scope::User, &spec(true), &home_env, None).unwrap();
        let text = &with.files[0].1;
        assert!(
            text.contains("PartOf=graphical-session.target\n")
                && text.contains("After=graphical-session.target\n")
                && text.contains("WantedBy=graphical-session.target\n"),
            "{text}"
        );
        assert!(
            text.contains("After=network-online.target\n")
                && text.contains("Wants=network-online.target\n"),
            "the network lines stay: {text}"
        );
        let without = install_plan(Os::Unix, Scope::User, &spec(false), &home_env, None).unwrap();
        let text = &without.files[0].1;
        assert!(!text.contains("graphical-session"), "{text}");
        assert!(text.contains("WantedBy=default.target\n"), "{text}");
    }

    #[test]
    fn every_systemd_unit_stops_retrying_after_five_failures() {
        for scope in [Scope::User, Scope::System] {
            let plan = install_plan(Os::Unix, scope, &spec(false), &home_env, None).unwrap();
            let text = &plan.files[0].1;
            assert!(
                text.contains("StartLimitIntervalSec=60\n") && text.contains("StartLimitBurst=5\n"),
                "{text}"
            );
        }
    }

    #[test]
    fn systemd_quoting_escapes_specifiers_and_quotes() {
        assert_eq!(
            systemd_quote(Path::new("/opt/100%/ru\"rge$")),
            "\"/opt/100%%/ru\\\"rge$$\""
        );
        // real backslashes are each doubled
        assert_eq!(
            systemd_quote(Path::new(r"C:\dir\rurge")),
            r#""C:\\dir\\rurge""#
        );
        // order-sensitive: a backslash immediately followed by a quote. Escaping
        // the quote before doubling the backslash would instead double the
        // backslash the quote-escape itself introduced, corrupting the output.
        assert_eq!(systemd_quote(Path::new("a\\\"b")), "\"a\\\\\\\"b\"");
    }

    #[test]
    fn launchd_agent_and_daemon() {
        let plan = install_plan(Os::MacOs, Scope::User, &spec(true), &home_env, Some(501)).unwrap();
        let plist = PathBuf::from("/home/me")
            .join("Library")
            .join("LaunchAgents")
            .join("io.rurge.daemon.plist");
        assert_eq!(plan.files[0].0, plist);
        let text = &plan.files[0].1;
        assert!(
            text.contains("<key>Label</key>\n    <string>io.rurge.daemon</string>"),
            "{text}"
        );
        assert!(
            text.contains("<string>/home/me/my profiles/rurge.conf</string>"),
            "{text}"
        );
        assert!(text.contains("<string>--system-proxy</string>"));
        assert!(
            text.contains("<key>RunAtLoad</key>\n    <true/>")
                && text.contains("<key>SuccessfulExit</key>"),
            "{text}"
        );
        assert_eq!(
            plan.commands,
            [vec![
                "launchctl".to_string(),
                "bootstrap".to_string(),
                "gui/501".to_string(),
                plist.display().to_string(),
            ]]
        );
        let err = install_plan(Os::MacOs, Scope::User, &spec(false), &home_env, None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let daemon = install_plan(Os::MacOs, Scope::System, &spec(false), &home_env, None).unwrap();
        assert_eq!(
            daemon.files[0].0,
            PathBuf::from("/Library/LaunchDaemons").join("io.rurge.daemon.plist")
        );
        assert_eq!(
            daemon.commands[0][..3],
            ["launchctl", "bootstrap", "system"]
        );
        let removal = uninstall_plan(Os::MacOs, Scope::User, &home_env, Some(501)).unwrap();
        assert_eq!(
            removal.commands[0][..3],
            ["launchctl", "bootout", "gui/501"]
        );
        assert_eq!(removal.remove, [plist]);
    }

    #[test]
    fn plist_strings_are_xml_escaped() {
        let odd = ServiceSpec {
            config: PathBuf::from("/tmp/a&b<c>.conf"),
            ..spec(false)
        };
        let plan = install_plan(Os::MacOs, Scope::System, &odd, &home_env, None).unwrap();
        assert!(
            plan.files[0]
                .1
                .contains("<string>/tmp/a&amp;b&lt;c&gt;.conf</string>")
        );
    }

    #[test]
    fn windows_logon_task() {
        let spec = ServiceSpec {
            exe: PathBuf::from(r"C:\Program Files\rurge\rurge.exe"),
            config: PathBuf::from(r"C:\Users\me\rurge.conf"),
            system_proxy: true,
        };
        let plan = install_plan(Os::Windows, Scope::User, &spec, &home_env, None).unwrap();
        assert!(plan.files.is_empty());
        assert_eq!(
            plan.commands,
            [vec![
                "schtasks", "/create", "/tn", "rurge", "/sc", "onlogon", "/tr",
                r#""C:\Program Files\rurge\rurge.exe" run -c "C:\Users\me\rurge.conf" --system-proxy"#,
                "/f",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()]
        );
        let same = install_plan(Os::Windows, Scope::System, &spec, &home_env, None).unwrap();
        assert_eq!(same, plan, "the scope makes no difference on Windows");
        let removal = uninstall_plan(Os::Windows, Scope::User, &home_env, None).unwrap();
        assert_eq!(lines(&removal.commands), ["schtasks /delete /tn rurge /f"]);
    }

    #[test]
    fn execute_writes_runs_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let unit = dir.path().join("nested").join("rurge.service");
        let install = Plan {
            files: vec![(unit.clone(), "unit text".to_string())],
            commands: vec![vec!["systemctl".to_string(), "enable".to_string()]],
            remove: Vec::new(),
        };
        let runner = FakeRunner::default();
        execute(&install, &runner).unwrap();
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), "unit text");
        assert_eq!(runner.calls(), ["systemctl enable"]);
        // an install stops at a failing command
        let failing = FakeRunner::default();
        failing.fail("systemctl enable", "access denied");
        let err = execute(&install, &failing).unwrap_err();
        assert!(err.to_string().contains("access denied"));
        // an uninstall still removes its files when the command fails
        let uninstall = Plan {
            files: Vec::new(),
            commands: vec![vec!["systemctl".to_string(), "disable".to_string()]],
            remove: vec![unit.clone(), dir.path().join("never-existed")],
        };
        let failing = FakeRunner::default();
        failing.fail("systemctl disable", "not loaded");
        let err = execute(&uninstall, &failing).unwrap_err();
        assert!(err.to_string().contains("not loaded"));
        assert!(
            !unit.exists(),
            "the unit is gone although the command failed"
        );
    }

    #[test]
    fn execute_reports_the_directory_it_could_not_create() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let install = Plan {
            files: vec![(blocker.join("rurge.service"), "unit text".to_string())],
            commands: vec![vec!["systemctl".to_string(), "enable".to_string()]],
            remove: Vec::new(),
        };
        let runner = FakeRunner::default();
        let err = execute(&install, &runner).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("cannot create directory"), "{message}");
        assert!(
            message.contains(&blocker.display().to_string()),
            "{message}"
        );
        assert!(runner.calls().is_empty());
    }
}
