//! Running external tools (`networksetup`, `gsettings`, `systemctl`, …) behind
//! a trait, so the code that decides *what* to run is tested without running it.

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
        Err(io::Error::other(format!(
            "`{}` failed ({}): {detail}",
            cmd.join(" "),
            output.status
        )))
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
