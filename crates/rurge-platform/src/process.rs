//! Stopping an external program together with every process it started
//! (phase 2 M4 design 7.3, M4-D6). On Unix the program leads a process group
//! of its own, and the group is signalled. On Windows it runs in a Job Object
//! that ends every process in it when the job's handle closes — when rurge
//! closes it, and also when rurge dies.

use std::io;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
use std::process::Command;

/// Called on the command before it is spawned: on Unix the program becomes
/// the leader of a new process group. On Windows it starts a new process
/// group of its own, so the Ctrl-C typed at rurge's console does not reach
/// it: rurge stops it in its own time, as it does on Unix.
pub fn prepare(command: &mut Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(
        command,
        windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
    );
}

/// The program and whatever it starts.
pub struct ProcessTree {
    #[cfg(unix)]
    group: nix::unistd::Pid,
    /// `None` once closed: every process of the tree has been ended.
    #[cfg(windows)]
    job: Option<std::os::windows::io::OwnedHandle>,
}

impl ProcessTree {
    /// Takes in the program `pid`, spawned after `prepare` and not waited for
    /// yet (so the id is still its own). On Windows a process the program
    /// started before this call is not taken in. 0 and 1 are never a program
    /// rurge started (on Unix, a group of 0 would be rurge's own).
    pub fn contain(pid: u32) -> io::Result<ProcessTree> {
        if pid <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a process rurge started",
            ));
        }
        #[cfg(unix)]
        {
            let pid = i32::try_from(pid)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "no such process id"))?;
            Ok(ProcessTree {
                group: nix::unistd::Pid::from_raw(pid),
            })
        }
        #[cfg(windows)]
        {
            Ok(ProcessTree {
                job: Some(windows::job_for(pid)?),
            })
        }
    }

    /// Asks every process of the tree to end: SIGTERM to the group on Unix.
    /// Windows has no asking: every process ends at once.
    pub fn terminate(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.signal(nix::sys::signal::Signal::SIGTERM)
        }
        #[cfg(windows)]
        {
            self.job = None;
            Ok(())
        }
    }

    /// Ends every process of the tree now: SIGKILL to the group on Unix; on
    /// Windows the same as `terminate`.
    pub fn kill(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.signal(nix::sys::signal::Signal::SIGKILL)
        }
        #[cfg(windows)]
        {
            self.terminate()
        }
    }

    /// Whether process `pid` is the program or one it started that is still
    /// in its tree (M4 design 11). `false` for a process that is not running,
    /// and for a tree that has already been ended.
    pub fn contains(&self, pid: u32) -> bool {
        // the two that are never a program rurge started, and a group of 0
        // would be rurge's own (see `contain`)
        if pid <= 1 {
            return false;
        }
        #[cfg(unix)]
        {
            let Ok(pid) = i32::try_from(pid) else {
                return false;
            };
            // the program leads the group of everything it started, which
            // inherit it; a process that is gone is not in it (`ESRCH`)
            nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(pid))) == Ok(self.group)
        }
        #[cfg(windows)]
        {
            // every process the program started while the job was open is in
            // the job; ours is not
            let Some(job) = self.job.as_ref() else {
                return false;
            };
            match windows::in_job(job.as_raw_handle(), pid) {
                Ok(contained) => contained,
                Err(e) => {
                    tracing::debug!("whether {pid} is in the program's job is unknown: {e}");
                    false
                }
            }
        }
    }

    /// A group that has already ended is not an error.
    #[cfg(unix)]
    fn signal(&self, signal: nix::sys::signal::Signal) -> io::Result<()> {
        match nix::sys::signal::killpg(self.group, signal) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
            // Darwin answers EPERM, not ESRCH, for a group whose processes
            // have all exited but are not reaped yet: it has ended too
            #[cfg(target_os = "macos")]
            Err(nix::errno::Errno::EPERM) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

    /// A Job Object that ends every process in it when its last handle
    /// closes, with process `pid` in it. One of the functions on the list in
    /// `tests/unsafe_functions.rs` that may use `unsafe` (M4-D6): nothing but
    /// the four calls, on handles this function owns.
    #[allow(unsafe_code)]
    pub(super) fn job_for(pid: u32) -> io::Result<OwnedHandle> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        // SAFETY: no security attributes and no name are documented as
        // valid; a non-null result is a new handle nobody else owns.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `job` is a valid handle owned by nothing else.
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the buffer is the structure the class names, with its own
        // size, and lives across the call.
        let set = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: plain call; a non-null result is a new handle nobody else owns.
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `process` is a valid handle owned by nothing else.
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        // SAFETY: both handles are valid and outlive the call.
        let assigned =
            unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Whether process `pid` is in `job` (M4 design 11): `OpenProcess` with
    /// `PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE`, a zero-wait
    /// `WaitForSingleObject` (an exited process is not in it, whatever
    /// `IsProcessInJob` would say of it), then `IsProcessInJob`. A process that
    /// is gone is not in it.
    #[allow(unsafe_code)]
    pub(super) fn in_job(job: RawHandle, pid: u32) -> io::Result<bool> {
        use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            WaitForSingleObject,
        };
        // SAFETY: plain call; a non-null result is a new handle nobody else owns.
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if process.is_null() {
            let error = io::Error::last_os_error();
            // a process that is gone, rather than one we may not ask about
            if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                return Ok(false);
            }
            return Err(error);
        }
        // SAFETY: `process` is a valid handle owned by nothing else.
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        // SAFETY: a valid handle that outlives the call, and a zero timeout
        // never waits.
        let waited = unsafe { WaitForSingleObject(process.as_raw_handle(), 0) };
        if waited == WAIT_OBJECT_0 {
            return Ok(false);
        }
        let mut contained = 0;
        // SAFETY: both handles are valid and outlive the call, and `contained`
        // is a `BOOL` that lives across it.
        let asked = unsafe { IsProcessInJob(process.as_raw_handle(), job, &mut contained) };
        if asked == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(contained != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    /// A program that runs for half a minute unless ended, started through
    /// a shell, so the shell's own child is a grandchild of ours.
    fn long_runner() -> Command {
        #[cfg(windows)]
        let mut command = {
            let mut c = Command::new("cmd");
            c.args(["/c", "ping -n 30 127.0.0.1"]);
            c
        };
        #[cfg(unix)]
        let mut command = {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 30; sleep 30"]);
            c
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        prepare(&mut command);
        command
    }

    fn ended_within(child: &mut Child, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn terminating_the_tree_ends_the_program() {
        let mut child = long_runner().spawn().unwrap();
        let mut tree = ProcessTree::contain(child.id()).unwrap();
        assert!(!ended_within(&mut child, Duration::from_millis(200)));
        tree.terminate().unwrap();
        let ended = ended_within(&mut child, Duration::from_secs(10));
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the program is still running");
        // ending an ended tree is no error
        tree.terminate().unwrap();
        tree.kill().unwrap();
    }

    #[test]
    fn killing_the_tree_ends_the_program() {
        let mut child = long_runner().spawn().unwrap();
        let mut tree = ProcessTree::contain(child.id()).unwrap();
        tree.kill().unwrap();
        let ended = ended_within(&mut child, Duration::from_secs(10));
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the program is still running");
    }

    /// The tree is the program and what it started while they last, and
    /// nobody else (M4 design 11): a session of either is DIRECT. The shell
    /// leads a group of its own, and the sleep it puts in the background is
    /// what it started. Only CI's Linux and macOS run this.
    #[cfg(unix)]
    #[test]
    fn a_process_tree_contains_its_leader_and_what_it_started_and_nothing_else() {
        use std::io::{BufRead, BufReader};
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & echo $!; wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        prepare(&mut command);
        let mut child = command.spawn().unwrap();
        let leader = child.id();
        let printed = BufReader::new(child.stdout.take().unwrap())
            .lines()
            .next()
            .expect("the child's id is printed")
            .unwrap();
        let started: u32 = printed.trim().parse().unwrap();
        let mut tree = ProcessTree::contain(leader).unwrap();
        assert!(tree.contains(leader), "the program itself");
        assert!(tree.contains(started), "what the program started");
        assert!(!tree.contains(std::process::id()), "our own process");
        tree.kill().unwrap();
        let _ = child.wait();
    }

    /// The program itself is in the job, and our own process is not (M4
    /// design 11): a session of the program is DIRECT, ours follows the rules.
    #[cfg(windows)]
    #[test]
    fn a_child_in_the_job_is_contained_and_this_process_is_not() {
        let mut child = long_runner().spawn().unwrap();
        let mut tree = ProcessTree::contain(child.id()).unwrap();
        assert!(tree.contains(child.id()), "the program itself");
        assert!(!tree.contains(std::process::id()), "our own process");
        tree.kill().unwrap();
        let ended = ended_within(&mut child, Duration::from_secs(10));
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the program is still running");
    }

    /// An ended process is in nothing, even while its process object lives
    /// on: `Child` keeps the handle until it is dropped, so `OpenProcess`
    /// still opens it and `IsProcessInJob` still answers. The zero-wait
    /// `WaitForSingleObject` is what says it has ended.
    #[cfg(windows)]
    #[test]
    fn an_exited_child_whose_handle_is_still_open_is_not_contained() {
        let mut child = long_runner().spawn().unwrap();
        let mut tree = ProcessTree::contain(child.id()).unwrap();
        assert!(tree.contains(child.id()), "the program while it runs");
        tree.kill().unwrap();
        let ended = ended_within(&mut child, Duration::from_secs(10));
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the program is still running");
        child.wait().unwrap();
        assert!(!tree.contains(child.id()), "an ended process is in nothing");
    }

    /// A process that is gone out of the system is in nothing either:
    /// `OpenProcess` answers `ERROR_INVALID_PARAMETER` for its id.
    #[cfg(windows)]
    #[test]
    fn a_process_that_is_gone_is_not_contained() {
        let mut child = long_runner().spawn().unwrap();
        let mut tree = ProcessTree::contain(child.id()).unwrap();
        let pid = child.id();
        tree.kill().unwrap();
        let ended = ended_within(&mut child, Duration::from_secs(10));
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the program is still running");
        // the last handle to it: the process object goes with it
        drop(child);
        assert!(!tree.contains(pid), "a process that is gone is in nothing");
    }

    /// The job goes with its handle: a rurge that dies (or forgets the tree)
    /// leaves nothing behind (M4-D6).
    #[cfg(windows)]
    #[test]
    fn dropping_the_tree_ends_the_program_on_windows() {
        let mut child = long_runner().spawn().unwrap();
        let tree = ProcessTree::contain(child.id()).unwrap();
        drop(tree);
        let ended = ended_within(&mut child, Duration::from_secs(10));
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the program is still running");
    }

    /// 0 and 1 are never a program rurge started: on Unix, signalling a
    /// group of 0 would reach rurge's own group.
    #[test]
    fn the_first_two_process_ids_cannot_be_taken_in() {
        for pid in [0, 1] {
            let err = ProcessTree::contain(pid).err().expect("refused");
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{pid}");
        }
    }
}
