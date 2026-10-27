//! Running the steps a translation produces (design 5.1, 5.5): the only place
//! in `route` that starts a program, and it starts what it is given through the
//! [`CommandRunner`] it is handed, so a test runs nothing.
//!
//! A script goes to a file of its own under `scratch` — the data directory's
//! `enhanced/`, which belongs to whoever runs rurge (M4c plan P12): never to
//! `/tmp`, where another user could have put a link in its place. The name
//! carries the process and a number of its own, and the file is created with
//! `create_new`, so nothing that is already there is written into.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::steps::{OnFailure, SCRIPT, Step};
use crate::command::CommandRunner;

/// What makes a script's name its own: the process is in the name, and this
/// number tells the runs of that process apart.
static WRITTEN: AtomicU64 = AtomicU64::new(0);

/// What the name of a script file starts with.
const PREFIX: &str = "rurge-steps-";
/// What it ends with.
const SUFFIX: &str = ".txt";

/// Runs `steps` in order (M4 design 5.1). A step's script is written to a file
/// of its own under `scratch` (made when missing; `create_new`, named
/// `rurge-steps-<pid>-<n>.txt`), whose path takes the place of [`SCRIPT`] in the
/// arguments; the file is removed once the program has run.
///
/// A step that fails: `Fail` ends the run with its error (the steps after it do
/// not run); `Ipv6Off` skips the other `Ipv6Off` steps of this run and goes on,
/// with a DEBUG line for the failure — whether it costs the user IPv6 is the
/// caller's to say and to warn about (the manager for the adapter's address,
/// `run_routes` for the routes); `Ignore` is a debug line, and the run goes on.
/// `Ok(false)`: an `Ipv6Off` step failed.
pub fn run(steps: &[Step], runner: &dyn CommandRunner, scratch: &Path) -> Result<bool, String> {
    let mut ipv6 = true;
    for step in steps {
        if !ipv6 && step.on_failure == OnFailure::Ipv6Off {
            continue;
        }
        match run_step(step, runner, scratch) {
            Ok(()) => {}
            Err(error) => match step.on_failure {
                OnFailure::Fail => return Err(error),
                OnFailure::Ipv6Off => {
                    ipv6 = false;
                    tracing::debug!(error, "a step of the IPv6 half failed");
                }
                OnFailure::Ignore => {
                    tracing::debug!(error, "a step that may fail failed; going on");
                }
            },
        }
    }
    Ok(ipv6)
}

/// Removes the scripts a run that died left under `scratch`: only the plain
/// files directly there whose names a run of this module gives them. A
/// `scratch` that is not there holds none.
pub fn remove_leftovers(scratch: &Path) {
    let entries = match fs::read_dir(scratch) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::debug!(path = %scratch.display(), error = %e, "cannot read the script directory");
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(PREFIX) || !name.ends_with(SUFFIX) {
            continue;
        }
        // a directory or a link named like a script is not one of ours
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        if let Err(e) = fs::remove_file(entry.path()) {
            tracing::debug!(path = %entry.path().display(), error = %e, "cannot remove a script left behind");
        }
    }
}

/// Runs one step: writes its script, puts the file's path where [`SCRIPT`]
/// stands, says what is about to run, runs it, and removes the script again.
///
/// A scripted step that fails names no script path in its error: the path is
/// under the data directory, which may be under an account's home, and the
/// error goes to the log, `rurge run`'s standard error and the API's 500 —
/// while the DEBUG line above has the path and the file, which is for the log
/// alone (the phase 3 / M4c plan's execution record row 44; M4 design 13).
/// `ip -batch` puts the path in its own words too (`Command failed
/// <path>:<line>`), so every occurrence in the error is replaced, not only the
/// one in the command line.
fn run_step(step: &Step, runner: &dyn CommandRunner, scratch: &Path) -> Result<(), String> {
    let mut written = None;
    if let Some(text) = &step.script {
        let path = write_script(scratch, text).map_err(|e| cannot_write(step, &e))?;
        written = Some((path, text));
    }
    let mut cmd = Vec::with_capacity(step.args.len() + 1);
    cmd.push(step.program.to_string_lossy().into_owned());
    for arg in &step.args {
        cmd.push(match (&written, arg == SCRIPT) {
            (Some((path, _)), true) => path.to_string_lossy().into_owned(),
            _ => arg.clone(),
        });
    }
    tracing::debug!(
        program = %step.program.display(),
        args = %cmd[1..].join(" "),
        script = written.as_ref().map(|(_, text)| *text),
        "running a step of the enhanced mode"
    );
    let result = runner.run(&cmd).map(drop).map_err(|e| e.to_string());
    if let Some((path, _)) = &written {
        remove_script(path);
    }
    result.map_err(|error| match &written {
        Some((path, _)) => error.replace(path.to_string_lossy().as_ref(), "…"),
        None => error,
    })
}

/// Writes `text` to a file of its own under `scratch`, made when missing, and
/// gives its path. A name that is already taken is tried again with the next
/// number: a run that died may have left one behind.
fn write_script(scratch: &Path, text: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(scratch)?;
    loop {
        let n = WRITTEN.fetch_add(1, Ordering::Relaxed);
        let path = scratch.join(format!("{PREFIX}{}-{n}{SUFFIX}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(text.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Removes a script once its program has run. What cannot be removed is not an
/// error: it is a leftover, and the next open removes it.
fn remove_script(path: &Path) {
    if let Err(e) = fs::remove_file(path) {
        tracing::debug!(path = %path.display(), error = %e, "cannot remove a script");
    }
}

/// What a step whose script cannot be written fails with: the program by its
/// file name (`ip`), which is all of it the reader needs.
fn cannot_write(step: &Step, error: &io::Error) -> String {
    let program = step.program.file_name().map_or_else(
        || step.program.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    format!("cannot write a script for {program}: {error}")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::log_capture::LogCapture;

    /// A command a run executed, and what the program would have read: the
    /// script as it was on disk at that moment.
    #[derive(Clone, Debug)]
    struct Call {
        cmd: Vec<String>,
        script: String,
    }

    /// Runs nothing: it records what it is given, and fails the steps a case
    /// says to fail, by the program's file name and a piece of the script.
    #[derive(Default)]
    struct Recording {
        calls: Mutex<Vec<Call>>,
        failures: Mutex<Vec<(String, String, String)>>,
        /// Whether a failure echoes the command line and the tool's own words
        /// as `command::failure` and `ip -batch` do — script path and all
        /// (the phase 3 / M4c plan's execution record row 44).
        echo_script_path: AtomicBool,
    }

    impl Recording {
        /// Fails every step whose program is `program` by its file name and
        /// whose script holds `text` — nothing at all for a step without one.
        fn fail(&self, program: &str, text: &str, message: &str) {
            self.failures.lock().unwrap().push((
                program.to_owned(),
                text.to_owned(),
                message.to_owned(),
            ));
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for Recording {
        fn run(&self, cmd: &[String]) -> io::Result<String> {
            // What the program would read: the run put the path of the script
            // in the arguments, and the file is there until the program ends.
            let script = cmd
                .iter()
                .skip(1)
                .find_map(|arg| fs::read_to_string(arg).ok())
                .unwrap_or_default();
            self.calls.lock().unwrap().push(Call {
                cmd: cmd.to_vec(),
                script: script.clone(),
            });
            let program = Path::new(&cmd[0])
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let failures = self.failures.lock().unwrap();
            match failures
                .iter()
                .find(|(name, text, _)| *name == program && script.contains(text.as_str()))
            {
                Some((_, _, message)) => {
                    let message = if self.echo_script_path.load(Ordering::SeqCst) {
                        format!(
                            "`{}` failed (exit status: 1): Command failed {}:1",
                            cmd.join(" "),
                            cmd[2]
                        )
                    } else {
                        message.clone()
                    };
                    Err(io::Error::other(message))
                }
                None => Ok(String::new()),
            }
        }
    }

    /// A step whose script goes where [`SCRIPT`] stands in its arguments.
    fn scripted(program: &str, script: &str, on_failure: OnFailure) -> Step {
        Step {
            program: PathBuf::from(program),
            args: vec!["-batch".to_owned(), SCRIPT.to_owned()],
            script: Some(script.to_owned()),
            on_failure,
        }
    }

    fn plain(program: &str, args: &[&str], on_failure: OnFailure) -> Step {
        Step {
            program: PathBuf::from(program),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            script: None,
            on_failure,
        }
    }

    /// The script goes to a file of its own, whose path the program is given,
    /// and is gone once the program has run (design 5.1, M4c plan P12).
    #[test]
    fn a_script_is_written_to_a_file_of_its_own_and_removed_after() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("enhanced");
        let text = "route add 1.0.0.0/8 dev rurge\n";
        let steps = [scripted("/usr/sbin/ip", text, OnFailure::Fail)];
        let runner = Recording::default();

        assert_eq!(run(&steps, &runner, &scratch), Ok(true));

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].cmd[0], "/usr/sbin/ip");
        assert_eq!(calls[0].cmd[1], "-batch");
        let path = Path::new(&calls[0].cmd[2]);
        assert!(path.starts_with(&scratch), "{}", path.display());
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with(&format!("rurge-steps-{}-", std::process::id())),
            "{name}"
        );
        assert!(name.ends_with(".txt"), "{name}");
        // the program read the translation's text, word for word
        assert_eq!(calls[0].script, text);
        // and the file is gone once it has run
        assert!(!path.exists(), "{}", path.display());
        assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);

        // a second run does not take the same name
        assert_eq!(run(&steps, &runner, &scratch), Ok(true));
        let calls = runner.calls();
        assert_ne!(calls[1].cmd[2], calls[0].cmd[2]);
    }

    /// A step that fails with `Fail` ends the run with its own error: what
    /// comes after it does not run.
    #[test]
    fn the_first_failing_step_ends_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("enhanced");
        let steps = [
            scripted(
                "/usr/sbin/ip",
                "addr add 198.18.0.1/15 dev rurge\n",
                OnFailure::Fail,
            ),
            scripted(
                "/usr/sbin/ip",
                "route add 198.18.0.0/15 dev rurge\n",
                OnFailure::Fail,
            ),
            plain("/usr/sbin/resolvectl", &["flush-caches"], OnFailure::Fail),
        ];
        let runner = Recording::default();
        runner.fail("ip", "198.18.0.0/15", "`ip -batch …` failed: File exists");

        assert_eq!(
            run(&steps, &runner, &scratch),
            Err("`ip -batch …` failed: File exists".to_owned())
        );
        // the third step did not run
        assert_eq!(runner.calls().len(), 2);
    }

    /// A step of the IPv6 half that fails skips the other IPv6 steps of this
    /// open — the rest goes on, and the caller learns that IPv6 is off. The
    /// failure is a DEBUG line: whether it costs the user anything is the
    /// caller's to say (the manager for the address, `run_routes` for the
    /// routes), and it warns about it there.
    #[test]
    fn an_ipv6_failure_skips_the_other_ipv6_steps_and_goes_on() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("enhanced");
        let steps = [
            scripted(
                "/usr/sbin/ip",
                "addr add fd00:6152::1/64 dev rurge\n",
                OnFailure::Ipv6Off,
            ),
            scripted(
                "/usr/sbin/ip",
                "route add 100::/8 dev rurge\n",
                OnFailure::Ipv6Off,
            ),
            scripted(
                "/usr/sbin/ip",
                "route add 1.0.0.0/8 dev rurge\n",
                OnFailure::Fail,
            ),
        ];
        let runner = Recording::default();
        runner.fail("ip", "fd00:6152::1", "no IPv6 on this system");

        assert_eq!(run(&steps, &runner, &scratch), Ok(false));

        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert!(
            calls[0].script.contains("fd00:6152::1"),
            "{}",
            calls[0].script
        );
        assert!(
            calls[1].script.contains("route add 1.0.0.0/8"),
            "{}",
            calls[1].script
        );
        assert!(!calls.iter().any(|call| call.script.contains("100::/8")));

        let warns: Vec<String> = captured
            .lines()
            .into_iter()
            .filter(|line| line.starts_with("WARN"))
            .collect();
        assert!(warns.is_empty(), "the caller warns, not this: {warns:?}");
    }

    /// A step that fails with `Ignore` is a line in the log: the run goes on
    /// and nothing is off.
    #[test]
    fn ignored_failures_go_on() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("enhanced");
        let steps = [
            scripted(
                "/usr/sbin/ip",
                "route del 1.0.0.0/8 dev rurge\n",
                OnFailure::Ignore,
            ),
            scripted(
                "/usr/sbin/ip",
                "route del 2.0.0.0/8 dev rurge\n",
                OnFailure::Ignore,
            ),
        ];
        let runner = Recording::default();
        runner.fail("ip", "1.0.0.0/8", "no such route");

        assert_eq!(run(&steps, &runner, &scratch), Ok(true));
        assert_eq!(runner.calls().len(), 2);
    }

    /// A script that cannot be written fails its step, and the program is not
    /// run without it: there is nothing for it to read. The error names the
    /// program by its file name, not by its path.
    #[test]
    fn a_script_that_cannot_be_written_fails_its_step() {
        let dir = tempfile::tempdir().unwrap();
        // where the scripts go is a file: nothing can be written there
        let scratch = dir.path().join("enhanced");
        fs::write(&scratch, "").unwrap();
        let steps = [scripted(
            "/usr/sbin/ip",
            "addr add 198.18.0.1/15 dev rurge\n",
            OnFailure::Fail,
        )];
        let runner = Recording::default();

        let error = run(&steps, &runner, &scratch).unwrap_err();
        assert!(
            error.starts_with("cannot write a script for ip: "),
            "{error}"
        );
        assert!(!error.contains("/usr/sbin/ip"), "{error}");
        // and the program was not run
        assert!(runner.calls().is_empty());
    }

    /// A scripted step that fails names no script path in its error, wherever
    /// the path was: the command line's `-batch` argument and the tool's own
    /// `Command failed <path>:<line>` both stand as `…`, and the rest of both
    /// is kept (the phase 3 / M4c plan's execution record row 44).
    #[test]
    fn a_failing_script_leaves_its_path_out_of_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("scratch");
        let steps = [scripted(
            "/usr/sbin/ip",
            "route add 1.0.0.0/8 dev rurge\n",
            OnFailure::Fail,
        )];
        let runner = Recording::default();
        runner.fail("ip", "1.0.0.0/8", "the message is not used here");
        runner.echo_script_path.store(true, Ordering::SeqCst);

        let error = run(&steps, &runner, &scratch).unwrap_err();

        let path = scratch.display().to_string();
        assert!(
            !error.contains(&path),
            "the script's path is in it: {error}"
        );
        assert!(error.contains("-batch …"), "{error}");
        assert!(error.contains("Command failed …:1"), "{error}");
        assert!(error.contains("failed (exit status: 1)"), "{error}");
    }

    /// Only the files a run of its own left are removed: not the others, and
    /// not a directory that is named like one.
    #[test]
    fn leftover_scripts_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("enhanced");
        fs::create_dir_all(&scratch).unwrap();
        let one = scratch.join("rurge-steps-1-0.txt");
        let two = scratch.join("rurge-steps-2-7.txt");
        let other = scratch.join("state.json");
        for path in [&one, &two, &other] {
            fs::write(path, "route add 1.0.0.0/8 dev rurge\n").unwrap();
        }
        let named_like_one = scratch.join("rurge-steps-3-0.txt");
        fs::create_dir(&named_like_one).unwrap();

        remove_leftovers(&scratch);

        assert!(!one.exists() && !two.exists());
        assert!(other.exists(), "{}", other.display());
        assert!(named_like_one.is_dir(), "{}", named_like_one.display());

        // a place that is not there holds nothing to remove
        remove_leftovers(&dir.path().join("nowhere"));
    }
}
