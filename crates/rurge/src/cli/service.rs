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

fn carry_out(plan: &Plan, dry_run: bool, done: &str) -> ExitCode {
    if dry_run {
        print_plan(plan);
        return ExitCode::SUCCESS;
    }
    match execute(plan, &SystemRunner) {
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
            let spec = ServiceSpec {
                exe: std::env::current_exe().context("cannot locate the rurge binary")?,
                // `absolute`, not `canonicalize`: no `\\?\` prefix on Windows
                config: std::path::absolute(&install.config)
                    .with_context(|| format!("cannot resolve {}", install.config.display()))?,
                system_proxy: install.system_proxy,
            };
            let plan = install_plan(os, scope, &spec, &env, uid(os, scope))?;
            let done = match scope {
                Scope::User => "installed: rurge starts automatically (per user)",
                Scope::System => "installed: rurge starts automatically (system-wide)",
            };
            Ok(carry_out(&plan, install.dry_run, done))
        }
        ServiceCommand::Uninstall(uninstall) => {
            let scope = scope(uninstall.user);
            let plan = uninstall_plan(os, scope, &env, uid(os, scope))?;
            Ok(carry_out(&plan, uninstall.dry_run, "uninstalled"))
        }
    }
}
