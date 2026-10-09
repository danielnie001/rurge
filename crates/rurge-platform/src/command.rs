//! Running external tools (`networksetup`, `gsettings`, `systemctl`, …) behind
//! a trait, so the code that decides *what* to run is tested without running it.

use std::fmt;
use std::io;

/// One external command: `cmd[0]` is the program.
pub type Cmd = Vec<String>;

pub trait CommandRunner: Send + Sync {
    /// Runs the command; stdout on success, otherwise an error that carries
    /// what the tool printed.
    fn run(&self, cmd: &[String]) -> io::Result<String>;
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, cmd: &[String]) -> io::Result<String> {
        let (program, args) = cmd
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|e| io::Error::new(e.kind(), format!("cannot run {program}: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(stdout);
        }
        // `networksetup` reports its errors on stdout
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        Err(io::Error::other(failure(cmd, output.status, detail)))
    }
}

/// What the error of a command that failed says: the command, how it ended
/// and what the tool printed. The command is shown without the credentials
/// an argument may carry: a proxy setting written back can be a URL with a
/// user name and a password, and the error goes to the log.
fn failure(cmd: &[String], status: impl fmt::Display, detail: &str) -> String {
    let shown: Vec<String> = cmd.iter().map(|arg| without_credentials(arg)).collect();
    format!("`{}` failed ({status}): {detail}", shown.join(" "))
}

/// `arg` without what stands before its last `@` — a URL's user name and
/// password, which may hold an `@` themselves; what can be a scheme stays in
/// front. An argument without an `@` is shown whole.
fn without_credentials(arg: &str) -> String {
    let Some((before, rest)) = arg.rsplit_once('@') else {
        return arg.to_string();
    };
    let scheme = before.split_once("://").map(|(scheme, _)| scheme);
    let is_scheme = |s: &&str| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    };
    match scheme.filter(is_scheme) {
        Some(scheme) => format!("{scheme}://***@{rest}"),
        None => format!("***@{rest}"),
    }
}

/// Runs every command, stopping at the first failure.
pub fn run_all(runner: &dyn CommandRunner, cmds: &[Cmd]) -> io::Result<()> {
    cmds.iter().try_for_each(|c| runner.run(c).map(drop))
}

/// Runs every command even when some fail (undoing things should get as far
/// as it can); the first error is returned.
pub fn run_best_effort(runner: &dyn CommandRunner, cmds: &[Cmd]) -> io::Result<()> {
    let mut first_error = None;
    for c in cmds {
        if let Err(e) = runner.run(c) {
            tracing::warn!(error = %e, "command failed; continuing");
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::CommandRunner;
    use std::collections::HashMap;
    use std::io;
    use std::sync::Mutex;

    /// Records every command. Replies are looked up by the space-joined
    /// command; anything not listed succeeds with empty output.
    #[derive(Default)]
    pub struct FakeRunner {
        pub calls: Mutex<Vec<String>>,
        replies: Mutex<HashMap<String, Result<String, String>>>,
    }

    impl FakeRunner {
        pub fn reply(&self, cmd: &str, stdout: &str) {
            self.replies
                .lock()
                .unwrap()
                .insert(cmd.to_string(), Ok(stdout.to_string()));
        }
        pub fn fail(&self, cmd: &str, message: &str) {
            self.replies
                .lock()
                .unwrap()
                .insert(cmd.to_string(), Err(message.to_string()));
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, cmd: &[String]) -> io::Result<String> {
            let line = cmd.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            match self.replies.lock().unwrap().get(&line) {
                Some(Ok(stdout)) => Ok(stdout.clone()),
                Some(Err(message)) => Err(io::Error::other(message.clone())),
                None => Ok(String::new()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(args: &[&str]) -> Cmd {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    /// Restoring a system proxy writes the old settings back, and KDE keeps
    /// a proxy as a URL, which may carry a user name and a password: the
    /// error of a command that failed — it goes to the log and to standard
    /// error — names the command without them. What the tool printed is the
    /// tool's.
    #[test]
    fn a_failed_command_is_named_without_its_credentials() {
        let text = failure(
            &cmd(&[
                "kwriteconfig6",
                "--file",
                "kioslaverc",
                "--group",
                "Proxy Settings",
                "--key",
                "httpProxy",
                "http://user:s3cret@proxy.example 8080",
            ]),
            "exit status: 1",
            "cannot write to a@b",
        );
        assert_eq!(
            text,
            "`kwriteconfig6 --file kioslaverc --group Proxy Settings --key httpProxy \
             http://***@proxy.example 8080` failed (exit status: 1): cannot write to a@b"
        );
        for gone in ["user", "s3cret"] {
            assert!(!text.contains(gone), "{gone}: {text}");
        }
        // the last `@` ends the credentials, whatever they hold; without a
        // scheme in front nothing of them stays
        for (arg, shown) in [
            ("socks://al1ce:p@ss@[::1] 1080", "socks://***@[::1] 1080"),
            ("al1ce:p://w@proxy.example", "***@proxy.example"),
            ("al1ce@proxy.example", "***@proxy.example"),
        ] {
            assert_eq!(
                failure(&cmd(&["tool", arg]), 2, ""),
                format!("`tool {shown}` failed (2): "),
                "{arg}"
            );
        }
    }

    /// Without an `@` the command is named as it was run.
    #[test]
    fn a_failed_command_without_credentials_is_named_whole() {
        assert_eq!(
            failure(
                &cmd(&["networksetup", "-setwebproxy", "Wi-Fi", "127.0.0.1", "6152"]),
                "exit status: 4",
                "Wi-Fi is not a recognized network service."
            ),
            "`networksetup -setwebproxy Wi-Fi 127.0.0.1 6152` failed (exit status: 4): \
             Wi-Fi is not a recognized network service."
        );
    }
}
