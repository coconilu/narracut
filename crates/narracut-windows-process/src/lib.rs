#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
mod windows {
    use std::{
        io,
        os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
    };

    use windows_sys::Win32::{
        Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };

    /// An owned, wait-only process handle used to prove that a known Windows process terminated.
    #[derive(Debug)]
    pub struct ProcessTerminationBarrier {
        pid: u32,
        handle: OwnedHandle,
    }

    impl ProcessTerminationBarrier {
        /// Opens a non-inheritable `SYNCHRONIZE` handle for `pid`.
        pub fn open(pid: u32) -> io::Result<Self> {
            // SAFETY: `OpenProcess` receives a numeric PID, requests only the documented
            // `PROCESS_SYNCHRONIZE` access right, and asks for a non-inheritable owned handle.
            let raw_handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            if raw_handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: a non-null successful `OpenProcess` result transfers one owned handle.
            // `OwnedHandle` closes it exactly once on drop and is never reconstructed elsewhere.
            let handle = unsafe { OwnedHandle::from_raw_handle(raw_handle) };
            Ok(Self { pid, handle })
        }

        /// Returns the PID whose termination this handle observes.
        pub const fn pid(&self) -> u32 {
            self.pid
        }

        /// Checks the process handle without blocking.
        pub fn is_signaled(&self) -> io::Result<bool> {
            // SAFETY: `self.handle` remains valid and owned for this call; a zero timeout makes
            // this a non-blocking observation and does not mutate or close the handle.
            let result = unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) };
            match result {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                WAIT_FAILED => Err(io::Error::last_os_error()),
                other => Err(io::Error::other(format!(
                    "WaitForSingleObject returned unexpected status {other}"
                ))),
            }
        }
    }
}

#[cfg(windows)]
pub use windows::ProcessTerminationBarrier;

#[cfg(all(test, windows))]
mod tests {
    use std::{
        process::{Command, Stdio},
        thread,
        time::Duration,
    };

    use super::ProcessTerminationBarrier;

    const CHILD_TEST: &str = "tests::barrier_child_process";

    #[test]
    #[ignore = "spawned explicitly by child_exit_signals_the_barrier"]
    fn barrier_child_process() {
        thread::sleep(Duration::from_millis(250));
    }

    #[test]
    fn current_process_is_not_signaled() {
        let barrier = ProcessTerminationBarrier::open(std::process::id())
            .expect("open current process synchronization handle");
        assert_eq!(barrier.pid(), std::process::id());
        assert!(!barrier.is_signaled().expect("query current process"));
    }

    #[test]
    fn child_exit_signals_the_barrier() {
        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--ignored", "--exact", CHILD_TEST])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn barrier child");
        let barrier =
            ProcessTerminationBarrier::open(child.id()).expect("open child process handle");
        assert!(!barrier.is_signaled().expect("query live child"));
        assert!(child.wait().expect("wait barrier child").success());
        assert!(barrier.is_signaled().expect("query exited child"));
    }

    #[test]
    fn invalid_pid_is_rejected() {
        assert!(ProcessTerminationBarrier::open(0).is_err());
    }
}
