//! `rurge service install | uninstall` (M4 design §8.3): register rurge with
//! systemd / launchd / the Windows task scheduler so it starts automatically.

use anyhow::Context;
use clap::{Args, Subcommand};
use rurge_platform::command::{CommandRunner, SystemRunner};
use rurge_platform::dirs::Os;
use rurge_platform::service::{Plan, Scope, ServiceSpec, execute, install_plan, uninstall_plan};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommand,
}

#[derive(Subcommand)]
pub enum ServiceCommand {
    /// Start rurge automatically (systemd unit, launchd plist or a Windows logon task)
    Install(InstallArgs),
    /// Remove what `service install` registered
    Uninstall(UninstallArgs),
}

#[derive(Args)]
pub struct InstallArgs {
    /// Profile the service runs with
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Install for the current user instead of system-wide (Windows: always per user)
    #[arg(long)]
    pub user: bool,
    /// Start the service with --system-proxy
    #[arg(long)]
    pub system_proxy: bool,
    /// Print the files and commands without changing anything
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct UninstallArgs {
    /// Remove the per-user installation instead of the system-wide one
    #[arg(long)]
    pub user: bool,
    /// Print the commands without changing anything
    #[arg(long)]
    pub dry_run: bool,
}

fn scope(user: bool) -> Scope {
    if user { Scope::User } else { Scope::System }
}

/// launchd's per-user domain is `gui/<uid>`; `id -u` avoids an FFI call.
fn uid(os: Os, scope: Scope) -> Option<u32> {
    if os != Os::MacOs || scope != Scope::User {
        return None;
    }
    let out = SystemRunner
        .run(&["id".to_string(), "-u".to_string()])
        .ok()?;
    out.trim().parse().ok()
}

fn print_plan(plan: &Plan) {
    for (path, content) in &plan.files {
        println!("write {}:", path.display());
        print!("{content}");
    }
    for command in &plan.commands {
        println!("run: {}", command.join(" "));
    }
    for path in &plan.remove {
        println!("remove {}", path.display());
    }
    println!("dry run: nothing was changed");
}

/// launchd's per-user domain is `gui/<uid>`, and `gui/0` is root's session
/// rather than the user's: `--user` under `sudo` would register the agent
/// where the desktop never starts it.
fn check_user_scope(scope: Scope, uid: Option<u32>) -> anyhow::Result<()> {
    if scope == Scope::User && uid == Some(0) {
        anyhow::bail!("--user under sudo would target root's session; run it without sudo");
    }
    Ok(())
}

/// `dry_run` is all that stands between a test and a real `schtasks /create`
/// or `systemctl enable`, so the runner is injected and the guard is tested.
fn carry_out(plan: &Plan, dry_run: bool, done: &str, runner: &dyn CommandRunner) -> ExitCode {
    if dry_run {
        print_plan(plan);
        return ExitCode::SUCCESS;
    }
    match execute(plan, runner) {
        Ok(()) => {
            println!("{done}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

pub fn run(args: ServiceArgs) -> anyhow::Result<ExitCode> {
    let os = Os::current();
    let env = |key: &str| std::env::var_os(key);
    match args.command {
        ServiceCommand::Install(install) => {
            if !install.config.is_file() {
                anyhow::bail!("cannot find the profile {}", install.config.display());
            }
            let scope = scope(install.user);
            let uid = uid(os, scope);
            check_user_scope(scope, uid)?;
            let spec = ServiceSpec {
                exe: std::env::current_exe().context("cannot locate the rurge binary")?,
                // `absolute`, not `canonicalize`: no `\\?\` prefix on Windows
                config: std::path::absolute(&install.config)
                    .with_context(|| format!("cannot resolve {}", install.config.display()))?,
                system_proxy: install.system_proxy,
            };
            let plan = install_plan(os, scope, &spec, &env, uid)?;
            let done = match scope {
                Scope::User => "installed: rurge starts automatically (per user)",
                Scope::System => "installed: rurge starts automatically (system-wide)",
            };
            Ok(carry_out(&plan, install.dry_run, done, &SystemRunner))
        }
        ServiceCommand::Uninstall(uninstall) => {
            let scope = scope(uninstall.user);
            let uid = uid(os, scope);
            check_user_scope(scope, uid)?;
            let plan = uninstall_plan(os, scope, &env, uid)?;
            Ok(carry_out(
                &plan,
                uninstall.dry_run,
                "uninstalled",
                &SystemRunner,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records what it was asked to run, and optionally refuses.
    struct CountingRunner {
        calls: Mutex<Vec<String>>,
        fail: bool,
    }

    impl CountingRunner {
        fn new(fail: bool) -> CountingRunner {
            CountingRunner {
                calls: Mutex::new(Vec::new()),
                fail,
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for CountingRunner {
        fn run(&self, cmd: &[String]) -> std::io::Result<String> {
            self.calls.lock().unwrap().push(cmd.join(" "));
            if self.fail {
                return Err(std::io::Error::other("refused"));
            }
            Ok(String::new())
        }
    }

    /// `ExitCode` has no `PartialEq`; its `Debug` form is stable per platform.
    fn code(c: ExitCode) -> String {
        format!("{c:?}")
    }

    fn plan_in(dir: &std::path::Path) -> Plan {
        Plan {
            files: vec![(dir.join("rurge.service"), "unit text".to_string())],
            commands: vec![vec!["systemctl".to_string(), "enable".to_string()]],
            remove: Vec::new(),
        }
    }

    #[test]
    fn a_dry_run_runs_nothing_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_in(dir.path());
        let runner = CountingRunner::new(false);
        assert_eq!(
            code(carry_out(&plan, true, "done", &runner)),
            code(ExitCode::SUCCESS)
        );
        assert!(runner.calls().is_empty(), "a dry run runs no command");
        assert!(!plan.files[0].0.exists(), "a dry run writes no file");
    }

    #[test]
    fn a_real_run_writes_the_files_and_runs_the_commands_once() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_in(dir.path());
        let runner = CountingRunner::new(false);
        assert_eq!(
            code(carry_out(&plan, false, "done", &runner)),
            code(ExitCode::SUCCESS)
        );
        assert_eq!(runner.calls(), ["systemctl enable"]);
        assert_eq!(
            std::fs::read_to_string(&plan.files[0].0).unwrap(),
            "unit text"
        );
    }

    #[test]
    fn a_failing_command_is_not_a_success() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_in(dir.path());
        let runner = CountingRunner::new(true);
        assert_ne!(
            code(carry_out(&plan, false, "done", &runner)),
            code(ExitCode::SUCCESS)
        );
        assert_eq!(runner.calls(), ["systemctl enable"]);
    }

    #[test]
    fn user_scope_under_sudo_is_refused() {
        assert!(check_user_scope(Scope::User, Some(501)).is_ok());
        assert!(check_user_scope(Scope::User, None).is_ok());
        assert!(check_user_scope(Scope::System, Some(0)).is_ok());
        let err = check_user_scope(Scope::User, Some(0))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "--user under sudo would target root's session; run it without sudo"
        );
    }
}
