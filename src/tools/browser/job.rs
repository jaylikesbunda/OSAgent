//! Ties browser processes to OSA's lifetime on Windows.
//!
//! The browser is a child of the core. Its cleanup (`kill_on_drop`, the
//! `Drop` impl) only runs when the core exits cleanly. If the core crashes
//! or is killed, headless Chrome survives and keeps its debugging socket
//! open, which on the next start blocks the server port with a leaked
//! listener. Putting every browser into one job object with
//! `KILL_ON_JOB_CLOSE` makes Windows end the tree when the core's handle to
//! the job closes, however the core exits. The job is held in a static, so
//! its handle lives exactly as long as the process.

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;
    use tracing::warn;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// The job handle, stored as an integer so it can live in a static.
    static BROWSER_JOB: OnceLock<isize> = OnceLock::new();

    fn browser_job() -> Option<HANDLE> {
        let raw = *BROWSER_JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls. The job is created once and never closed
            // explicitly; the OS closes it when the process exits.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return 0;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 {
                    CloseHandle(job);
                    return 0;
                }
                job as isize
            }
        });
        if raw == 0 {
            None
        } else {
            Some(raw as HANDLE)
        }
    }

    /// Assign a freshly spawned browser process to the containment job.
    /// Failure only loses the crash-safety net; the normal teardown still runs.
    pub fn contain(pid: u32) {
        let Some(job) = browser_job() else {
            warn!(
                "browser containment job unavailable; a crashed OSA may leave the browser running"
            );
            return;
        };
        // SAFETY: the handles are checked for null and closed after use.
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                warn!(pid, "could not open browser process for containment");
                return;
            }
            if AssignProcessToJobObject(job, process) == 0 {
                warn!(pid, "could not assign browser process to containment job");
            }
            CloseHandle(process);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    /// Non-Windows builds rely on the process-group kill in `sandbox.rs`.
    pub fn contain(_pid: u32) {}
}

pub use imp::contain;

#[cfg(all(test, windows))]
mod tests {
    use super::contain;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;

    #[test]
    fn contained_process_is_in_the_browser_job() {
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a long-running child");
        contain(child.id());

        let mut in_job: i32 = 0;
        // SAFETY: the process handle stays valid until the child is killed below.
        let ok = unsafe {
            let process = child.as_raw_handle() as HANDLE;
            IsProcessInJob(process, std::ptr::null_mut(), &mut in_job)
        };
        let _ = child.kill();
        let _ = child.wait();
        assert_ne!(ok, 0, "IsProcessInJob failed");
        assert_ne!(
            in_job, 0,
            "contained browser process was not placed in the job"
        );
    }
}
