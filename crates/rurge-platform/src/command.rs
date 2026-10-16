//! Running external tools (`networksetup`, `gsettings`, `systemctl`, …) behind
//! a trait, so the code that decides *what* to run is tested without running it.

use std::fmt;
use std::io::{self, Read};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
/// front. An argument without an `@`, or with nothing in front of it but a
/// scheme (GNOME's empty list is `@as []`), is shown whole.
fn without_credentials(arg: &str) -> String {
    let Some((before, rest)) = arg.rsplit_once('@') else {
        return arg.to_string();
    };
    let scheme = before.split_once("://").map(|(scheme, _)| scheme);
    let is_scheme = |s: &&str| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    };
    let scheme = scheme.filter(is_scheme);
    let credentials = scheme.map_or(before, |scheme| &before[scheme.len() + "://".len()..]);
    if credentials.is_empty() {
        return arg.to_string();
    }
    match scheme {
        Some(scheme) => format!("{scheme}://***@{rest}"),
        None => format!("***@{rest}"),
    }
}

/// Runs a command like [`SystemRunner`] does, but gives up on it after
/// `timeout`: a program that has not ended by then is killed and the error is
/// [`io::ErrorKind::TimedOut`].
pub fn run_with_timeout(cmd: &[String], timeout: Duration) -> io::Result<String> {
    run_with_timeout_env(cmd, timeout, &[])
}

/// [`run_with_timeout`], with environment variables set for the program over
/// the ones it inherits.
pub(crate) fn run_with_timeout_env(
    cmd: &[String],
    timeout: Duration,
    envs: &[(&str, &str)],
) -> io::Result<String> {
    let (program, args) = cmd
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A console of its own, without a window: the program neither shows one
    // nor shares ours, so what it does to its console (`chcp`) stays with it.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("cannot run {program}: {e}")))?;
    let mut tree = Tree::of(&child);
    let deadline = Instant::now() + timeout;
    let stdout = read_to_end_on_a_thread(child.stdout.take());
    let stderr = read_to_end_on_a_thread(child.stderr.take());
    let left = || deadline.saturating_duration_since(Instant::now());
    let (stdout, stderr) = match (stdout.recv_timeout(left()), stderr.recv_timeout(left())) {
        (Ok(stdout), Ok(stderr)) => (stdout, stderr),
        _ => return Err(give_up(child, &mut tree, cmd, timeout)),
    };
    // The program has closed its output; it ends now, or soon.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(1)),
            Ok(None) | Err(_) => return Err(give_up(child, &mut tree, cmd, timeout)),
        }
    };
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    if status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&stderr);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    Err(io::Error::other(failure(cmd, status, detail)))
}

/// What a pipe holds until it is closed, read on a thread of its own so that a
/// program that writes more than a pipe takes is not left waiting for a reader.
/// A pipe that is not there is an empty one.
fn read_to_end_on_a_thread(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            // what was read before an error is all there is
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = tx.send(bytes);
    });
    rx
}

/// A program together with what it starts, as far as the system keeps them
/// together. On Windows a Job Object holds the program: `cmd` starts `netsh`,
/// and killing `cmd` alone leaves `netsh` where it is. Elsewhere the programs
/// asked about the network start nothing (`env` becomes the program it runs).
struct Tree(#[cfg(windows)] Option<crate::process::ProcessTree>);

impl Tree {
    fn of(child: &Child) -> Tree {
        #[cfg(windows)]
        {
            // a program that has ended already, or cannot be held, is killed alone
            Tree(crate::process::ProcessTree::contain(child.id()).ok())
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            Tree()
        }
    }

    fn kill(&mut self) {
        #[cfg(windows)]
        if let Some(tree) = self.0.as_mut() {
            let _ = tree.kill();
        }
    }
}

/// Kills a program that did not end in time, and what it started, and reaps
/// it. The threads reading its output end when its pipes close, unless a
/// program it started holds them open; nothing waits for them.
fn give_up(mut child: Child, tree: &mut Tree, cmd: &[String], timeout: Duration) -> io::Error {
    tree.kill();
    let _ = child.kill();
    let _ = child.wait();
    io::Error::new(
        io::ErrorKind::TimedOut,
        failure(
            cmd,
            "timed out",
            &format!("no answer within {} ms; killed", timeout.as_millis()),
        ),
    )
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
            // nothing in front of the `@` but a scheme: nothing is hidden, and
            // the command is shown as it ran (GNOME's empty list is `@as []`)
            ("@as []", "@as []"),
            ("@", "@"),
            ("http://@proxy.example", "http://@proxy.example"),
        ] {
            assert_eq!(
                failure(&cmd(&["tool", arg]), 2, ""),
                format!("`tool {shown}` failed (2): "),
                "{arg}"
            );
        }
    }

    /// The programs the tests of `run_with_timeout` run: the test binary
    /// itself, the one program every platform has and the only one this
    /// repository starts. The ignored tests below are its second half; they do
    /// nothing unless the test that starts them says so in the environment.
    fn this_binary() -> String {
        std::env::current_exe()
            .expect("the test binary has a path")
            .to_string_lossy()
            .into_owned()
    }

    fn run_ignored(
        name: &str,
        extra: &[&str],
        timeout: Duration,
        env: (&str, &str),
    ) -> io::Result<String> {
        let mut cmd = vec![
            this_binary(),
            "--ignored".to_string(),
            "--exact".to_string(),
            format!("command::tests::{name}"),
            "--nocapture".to_string(),
        ];
        cmd.extend(extra.iter().map(|arg| arg.to_string()));
        run_with_timeout_env(&cmd, timeout, &[env])
    }

    #[test]
    #[ignore = "started by a_program_that_does_not_end_is_killed_at_the_limit"]
    fn hold_until_killed() {
        if std::env::var_os("RURGE_TEST_HOLD").is_none_or(|value| value != "1") {
            return;
        }
        // wait for a message that never comes
        let (_open, never) = std::sync::mpsc::channel::<()>();
        let _ = never.recv();
    }

    #[test]
    #[ignore = "started by a_program_that_writes_a_lot_is_not_held_up"]
    fn chatter() {
        if std::env::var_os("RURGE_TEST_CHATTER").is_none_or(|value| value != "1") {
            return;
        }
        // two MiB on each of the two pipes: far more than a pipe holds
        let line = "x".repeat(1023);
        for _ in 0..2048 {
            println!("{line}");
            eprintln!("{line}");
        }
    }

    /// A program that ends in time gives what it wrote.
    #[test]
    fn a_program_that_ends_gives_its_output() {
        let out = run_with_timeout(
            &[this_binary(), "--list".to_string()],
            Duration::from_secs(60),
        )
        .expect("the test binary lists its tests");
        assert!(out.contains("hold_until_killed"), "{out}");
    }

    /// A program that does not end is killed when the time is up, and the
    /// error says so — without the credentials of its command line. The call
    /// comes back at all only because the program is gone: the last step is
    /// waiting for it, which a living one would not allow.
    #[test]
    fn a_program_that_does_not_end_is_killed_at_the_limit() {
        let limit = Duration::from_millis(500);
        let started = Instant::now();
        // a second filter on the command line, which the test binary reads as
        // one more test name and which stands for a credential
        let err = run_ignored(
            "hold_until_killed",
            &["http://user:s3cret@h.example"],
            limit,
            ("RURGE_TEST_HOLD", "1"),
        )
        .unwrap_err();
        let took = started.elapsed();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(took >= limit, "{took:?}");
        assert!(took < Duration::from_secs(30), "{took:?}");
        let text = err.to_string();
        assert!(text.contains("http://***@h.example"), "{text}");
        assert!(!text.contains("s3cret"), "{text}");
    }

    /// The output is read while the program runs: one that writes more than a
    /// pipe holds, on both of its pipes, is not left waiting for a reader.
    #[test]
    fn a_program_that_writes_a_lot_is_not_held_up() {
        let out = run_ignored(
            "chatter",
            &[],
            Duration::from_secs(60),
            ("RURGE_TEST_CHATTER", "1"),
        )
        .expect("the program ends in time");
        assert!(out.len() > 2 * 1024 * 1024, "{}", out.len());
    }

    /// Ending with a failure is an error that names the command without its
    /// credentials, as `SystemRunner`'s does.
    #[test]
    fn a_program_that_fails_is_named_without_its_credentials() {
        // the test binary refuses an option it does not know; the other
        // argument, which it would read as a test name, stands for a credential
        let err = run_with_timeout(
            &[
                this_binary(),
                "--no-such-option".to_string(),
                "http://user:s3cret@h.example".to_string(),
            ],
            Duration::from_secs(60),
        )
        .unwrap_err();
        let text = err.to_string();
        assert_ne!(err.kind(), io::ErrorKind::TimedOut, "{text}");
        assert!(text.contains("failed ("), "{text}");
        assert!(text.contains("http://***@h.example"), "{text}");
        assert!(!text.contains("s3cret"), "{text}");
    }

    #[test]
    fn a_program_that_is_not_there_is_not_found() {
        let err = run_with_timeout(
            &["rurge-no-such-program".to_string()],
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound, "{err}");
        assert!(
            err.to_string()
                .starts_with("cannot run rurge-no-such-program"),
            "{err}"
        );
    }

    #[test]
    fn an_empty_command_is_refused() {
        let err = run_with_timeout(&[], Duration::from_secs(5)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
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
