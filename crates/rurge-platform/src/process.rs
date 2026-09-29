//! Stopping an external program together with every process it started
//! (phase 2 M4 design 7.3, M4-D6). On Unix the program leads a process group
//! of its own, and the group is signalled. On Windows it runs in a Job Object
//! that ends every process in it when the job's handle closes — when rurge
//! closes it, and also when rurge dies.

use std::io;
use std::process::Command;

/// Called on the command before it is spawned: on Unix the program becomes
/// the leader of a new process group. Nothing on Windows.
pub fn prepare(command: &mut Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(not(unix))]
    let _ = command;
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
    /// started before this call is not taken in.
    pub fn contain(pid: u32) -> io::Result<ProcessTree> {
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

    /// A group that has already ended is not an error.
    #[cfg(unix)]
    fn signal(&self, signal: nix::sys::signal::Signal) -> io::Result<()> {
        match nix::sys::signal::killpg(self.group, signal) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    /// A Job Object that ends every process in it when its last handle
    /// closes, with process `pid` in it. The workspace's second `unsafe`
    /// (M4-D6): nothing but the four calls, on handles this function owns.
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

    #[cfg(windows)]
    #[test]
    fn a_process_that_does_not_exist_cannot_be_taken_in() {
        // process ids are multiples of 4 on Windows: 3 is never one
        assert!(ProcessTree::contain(3).is_err());
    }
}
