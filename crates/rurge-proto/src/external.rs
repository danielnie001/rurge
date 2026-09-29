//! `external` outbound (phase 2 M4 design §7): a program rurge starts itself
//! the first time the policy is used, reached as a SOCKS5 proxy at
//! `127.0.0.1:<local-port>`. A program that exited is started again on the
//! next use; the program and whatever it started are stopped together.

use crate::socks5::{connect_request, negotiate};
use crate::{Outbound, OutboundError};
use rurge_config::spec::ExternalSpec;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// Connection attempts per request, one every `ATTEMPT_EVERY` (manual).
pub const ATTEMPTS: u32 = 6;
pub const ATTEMPT_EVERY: Duration = Duration::from_millis(500);
/// Two starts of one policy's program are at least this far apart (M4-D11).
pub const START_GAP: Duration = Duration::from_secs(2);
/// How long a program asked to end may take before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(2);
/// A log larger than this is rotated before the next start.
pub const LOG_LIMIT: u64 = 1024 * 1024;

/// The proxy settings a program would otherwise follow back into rurge.
const PROXY_VARIABLES: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

/// How external programs are started and stopped: the bin injects
/// `rurge-platform::process` (a process group on Unix, a Job Object on
/// Windows), everything else uses `NoProcessGroups`.
pub trait ProcessHook: Send + Sync {
    /// Called on the command before it is spawned.
    fn prepare(&self, command: &mut Command);
    /// Takes in the program just spawned, with process id `pid`. `None`:
    /// only the program itself can be stopped, not what it started.
    fn contain(&self, pid: u32) -> io::Result<Option<Box<dyn ProcessGroup>>>;
}

/// A program and whatever it started.
pub trait ProcessGroup: Send {
    /// Asks every process to end.
    fn terminate(&mut self) -> io::Result<()>;
    /// Ends every process now.
    fn kill(&mut self) -> io::Result<()>;
}

/// No process groups: stopping a program kills the program alone.
pub struct NoProcessGroups;

impl ProcessHook for NoProcessGroups {
    fn prepare(&self, _command: &mut Command) {}

    fn contain(&self, _pid: u32) -> io::Result<Option<Box<dyn ProcessGroup>>> {
        Ok(None)
    }
}

/// `<policy>.log`: characters a file name cannot hold become `_`, and a
/// name that had to change gets a short hash of the original, so two
/// policies never share a log.
pub fn log_file_name(policy: &str) -> String {
    let mut stem: String = policy
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows drops trailing dots and spaces; a leading dot hides the file
    let trimmed = stem.trim_end_matches(['.', ' ']).trim_start_matches('.');
    if trimmed.len() != stem.len() {
        stem = trimmed.to_string();
    }
    if stem != policy || stem.is_empty() || reserved_on_windows(&stem) {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(policy.as_bytes());
        stem = format!(
            "{stem}-{:02x}{:02x}{:02x}{:02x}",
            hash[0], hash[1], hash[2], hash[3]
        );
    }
    format!("{stem}.log")
}

/// `CON`, `NUL`, `COM1` and the rest stay device names with an extension.
fn reserved_on_windows(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit())
}

/// Opens the log for one more start: a log over `LOG_LIMIT` becomes
/// `<name>.log.1` first (one old file is kept), then a separator line goes in.
fn open_log(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        std::fs::rename(path, old)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    writeln!(
        file,
        "--- rurge: starting the program (unix time {now}) ---"
    )?;
    Ok(file)
}

/// The command for one start: the arguments in order, no input, output to
/// the log, and no proxy settings to follow back into rurge (M4-D11).
fn command(exec: &str, args: &[String], log: &File) -> io::Result<Command> {
    let mut command = Command::new(exec);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?);
    for name in PROXY_VARIABLES {
        command.env_remove(name);
    }
    command.env("NO_PROXY", "*").env("no_proxy", "*");
    Ok(command)
}

/// A started program, watched by its own task.
struct Running {
    /// Dropped or sent: the task stops the program.
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

#[derive(Default)]
struct Slot {
    running: Option<Running>,
    /// When the program was last started (or failed to start).
    started: Option<Instant>,
    /// Why the last start failed, while `started` is recent.
    failed: Option<io::ErrorKind>,
}

pub struct ExternalOutbound {
    name: String,
    exec: String,
    args: Vec<String>,
    port: u16,
    log: PathBuf,
    hook: Arc<dyn ProcessHook>,
    slot: Mutex<Slot>,
}

fn start_failed(policy: &str, kind: io::ErrorKind) -> OutboundError {
    OutboundError::Proxy(format!("external: could not start {policy} ({kind})"))
}

impl ExternalOutbound {
    /// Nothing starts here: a build only checks (M4 design 7.5). The log
    /// goes to `log_dir`.
    pub fn new(
        name: &str,
        spec: &ExternalSpec,
        log_dir: &Path,
        hook: Arc<dyn ProcessHook>,
    ) -> ExternalOutbound {
        ExternalOutbound {
            name: name.to_string(),
            exec: spec.exec.clone(),
            args: spec.args.expose().clone(),
            port: spec.local_port,
            log: log_dir.join(log_file_name(name)),
            hook,
            slot: Mutex::new(Slot::default()),
        }
    }

    /// Where the program's output goes.
    pub fn log_path(&self) -> &Path {
        &self.log
    }

    /// Stops the program and whatever it started, when it runs; returns
    /// when they are gone (at most about `STOP_GRACE` later).
    pub async fn stop(&self) {
        let running = self.slot.lock().await.running.take();
        if let Some(Running { stop, task }) = running {
            let _ = stop.send(());
            let _ = task.await;
        }
    }

    /// Starts the program unless it runs. Within `START_GAP` of the last
    /// start nothing is started: a failed start is failed again, and a
    /// program that has exited since is left to the caller's retries.
    async fn ensure_started(&self) -> Result<(), OutboundError> {
        let mut slot = self.slot.lock().await;
        if slot
            .running
            .as_ref()
            .is_some_and(|running| !running.task.is_finished())
        {
            return Ok(());
        }
        slot.running = None;
        if let Some(at) = slot.started
            && at.elapsed() < START_GAP
        {
            return match slot.failed {
                Some(kind) => Err(start_failed(&self.name, kind)),
                None => Ok(()),
            };
        }
        slot.started = Some(Instant::now());
        match self.start() {
            Ok(running) => {
                slot.running = Some(running);
                slot.failed = None;
                Ok(())
            }
            Err(e) => {
                tracing::warn!(policy = %self.name, error = %e.kind(), "external: the program could not be started");
                slot.failed = Some(e.kind());
                Err(start_failed(&self.name, e.kind()))
            }
        }
    }

    fn start(&self) -> io::Result<Running> {
        let log = open_log(&self.log)?;
        let mut command = command(&self.exec, &self.args, &log)?;
        self.hook.prepare(&mut command);
        let mut command = tokio::process::Command::from(command);
        // the last resort, should the watching task be dropped unfinished
        command.kill_on_drop(true);
        let child = command.spawn()?;
        let pid = child.id().unwrap_or(0);
        // an error drops `child`, which kills it
        let group = self.hook.contain(pid)?;
        tracing::info!(policy = %self.name, pid, "external: the program started");
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(watch(self.name.clone(), pid, child, group, stopped));
        Ok(Running { stop, task })
    }

    async fn dial(&self, target: &Target) -> Result<BoxedStream, OutboundError> {
        // checked first: no program is started for a request we cannot send
        let request = connect_request(target)?;
        for attempt in 1..=ATTEMPTS {
            self.ensure_started().await?;
            // Windows takes about two seconds to refuse a connection to a
            // port nobody listens on: an attempt ends with its slot either way
            let slot_end = Instant::now() + ATTEMPT_EVERY;
            let connected = tokio::time::timeout_at(
                slot_end,
                TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)),
            )
            .await;
            match connected {
                Ok(Ok(stream)) => {
                    let _ = stream.set_nodelay(true);
                    return negotiate(Box::new(stream), &request, None).await;
                }
                Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => {}
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => {}
            }
            if attempt < ATTEMPTS {
                tokio::time::sleep_until(slot_end).await;
            }
        }
        Err(OutboundError::Proxy(
            "external: the local SOCKS5 port refused the connection".to_string(),
        ))
    }
}

/// Ends the group once, however the watching task ends: when it is dropped
/// unfinished (the runtime shutting down) too.
struct Tree(Option<Box<dyn ProcessGroup>>);

impl Tree {
    fn terminate(&mut self) {
        if let Some(group) = self.0.as_mut() {
            let _ = group.terminate();
        }
    }

    fn end(&mut self) {
        if let Some(mut group) = self.0.take() {
            let _ = group.kill();
        }
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        self.end();
    }
}

/// Owns the program until it exits or is told to stop. A program that
/// exits takes what it started with it: whatever is left of the group
/// would only hold the port the next start needs.
async fn watch(
    policy: String,
    pid: u32,
    mut child: tokio::process::Child,
    group: Option<Box<dyn ProcessGroup>>,
    stop: oneshot::Receiver<()>,
) {
    let mut tree = Tree(group);
    tokio::select! {
        status = child.wait() => {
            tree.end();
            let code = status.ok().and_then(|s| s.code());
            tracing::info!(policy = %policy, pid, code, "external: the program exited");
        }
        _ = stop => {
            if tree.0.is_some() {
                tree.terminate();
                if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_err() {
                    tree.end();
                }
            }
            tree.end();
            let _ = child.kill().await;
            tracing::info!(policy = %policy, pid, "external: the program stopped");
        }
    }
}

impl Outbound for ExternalOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.dial(target)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_is_named_after_its_policy() {
        assert_eq!(log_file_name("Home SSH"), "Home SSH.log");
        assert_eq!(log_file_name("香港 01"), "香港 01.log");
        // a changed name carries a hash of the original: never shared
        let a = log_file_name("a/b");
        let b = log_file_name("a:b");
        assert!(a.starts_with("a_b-") && a.ends_with(".log"), "{a}");
        assert!(b.starts_with("a_b-") && b != a, "{b}");
        assert_ne!(log_file_name("a_b"), a);
        for odd in [
            "..", "x.", " ", "CON", "nul", "com1", "LPT9", "con.x", "\u{7}",
        ] {
            let name = log_file_name(odd);
            assert!(name.len() > 4 + 8, "{odd:?} → {name}");
            assert!(!name.starts_with('.'), "{odd:?} → {name}");
        }
        assert_eq!(log_file_name("console"), "console.log");
        assert_eq!(log_file_name("COM10"), "COM10.log");
    }

    #[test]
    fn a_log_is_rotated_once_it_is_over_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("external").join("P.log");
        drop(open_log(&path).unwrap());
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(
            first.starts_with("--- rurge: starting the program (unix time "),
            "{first}"
        );
        assert_eq!(first.lines().count(), 1);
        // appended while it is small
        drop(open_log(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
        std::fs::write(&path, vec![b'x'; LOG_LIMIT as usize + 1]).unwrap();
        drop(open_log(&path).unwrap());
        let rotated = dir.path().join("external").join("P.log.1");
        assert_eq!(std::fs::metadata(&rotated).unwrap().len(), LOG_LIMIT + 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        // one old file only: the next rotation replaces it
        std::fs::write(&path, vec![b'y'; LOG_LIMIT as usize + 2]).unwrap();
        drop(open_log(&path).unwrap());
        assert_eq!(std::fs::metadata(&rotated).unwrap().len(), LOG_LIMIT + 2);
    }

    #[test]
    fn the_program_gets_no_proxy_settings() {
        let dir = tempfile::tempdir().unwrap();
        let log = File::create(dir.path().join("l")).unwrap();
        let command = command("prog", &["-D".into(), "1080".into()], &log).unwrap();
        assert_eq!(command.get_program(), "prog");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["-D", "1080"]);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for name in PROXY_VARIABLES {
            assert!(
                envs.iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case(name) && v.is_none()),
                "{name}: {envs:?}"
            );
        }
        assert!(
            envs.iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("NO_PROXY") && v.as_deref() == Some("*")),
            "{envs:?}"
        );
    }

    fn outbound(exec: &str, port: u16, dir: &Path) -> ExternalOutbound {
        let spec = ExternalSpec {
            exec: exec.to_string(),
            args: rurge_config::spec::Secret::new(Vec::new()),
            local_port: port,
            addresses: Vec::new(),
        };
        ExternalOutbound::new("P", &spec, dir, Arc::new(NoProcessGroups))
    }

    /// A program that cannot be started fails the request at once, and
    /// within `START_GAP` again without another try; the log was written.
    #[tokio::test]
    async fn a_program_that_cannot_start_fails_the_request() {
        let dir = tempfile::tempdir().unwrap();
        let o = outbound("./no-such-program-for-rurge", 9, dir.path());
        let target = Target::new(rurge_config::HostName::parse("t.test"), 80);
        let err = o
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .err()
            .unwrap();
        assert_eq!(
            err.to_string(),
            format!("external: could not start P ({})", io::ErrorKind::NotFound)
        );
        let at = o.slot.lock().await.started;
        let again = o
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .err()
            .unwrap();
        assert_eq!(again.to_string(), err.to_string());
        assert_eq!(
            o.slot.lock().await.started,
            at,
            "no second start within the gap"
        );
        assert!(o.log_path().exists());
    }

    /// Nothing starts for a request that cannot be sent.
    #[tokio::test]
    async fn an_unsendable_target_starts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let o = outbound("./no-such-program-for-rurge", 9, dir.path());
        let target = Target::new(rurge_config::HostName::parse(&"a".repeat(300)), 80);
        let err = o
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .err()
            .unwrap();
        assert_eq!(
            err.to_string(),
            "socks5: the host name is longer than 255 bytes"
        );
        assert!(o.slot.lock().await.started.is_none());
        assert!(!o.log_path().exists());
    }
}
