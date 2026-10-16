#[cfg(windows)]
pub(super) struct HostJob(windows_sys::Win32::Foundation::HANDLE);

// The unnamed kernel handle may move between threads. It is closed by its sole owner.
#[cfg(windows)]
unsafe impl Send for HostJob {}

#[cfg(windows)]
impl HostJob {
    pub fn terminate_and_wait(&self, timeout: std::time::Duration) -> std::io::Result<()> {
        use windows_sys::Win32::System::JobObjects::*;
        if unsafe { TerminateJobObject(self.0, 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            if unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut _,
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if info.ActiveProcesses == 0 {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(std::io::Error::other(
                    "Terminal job processes did not finish terminating.",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    /// The trusted host is blocked reading Start; it cannot spawn the user's shell yet.
    pub fn assign(child: &std::process::Child) -> std::io::Result<Self> {
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = Self(handle);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe { AssignProcessToJobObject(handle, child.as_raw_handle()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut member = 0;
        if unsafe { IsProcessInJob(child.as_raw_handle(), handle, &mut member) } == 0 || member == 0
        {
            return Err(std::io::Error::other(
                "Terminal host job membership was not verified.",
            ));
        }
        Ok(job)
    }
}

#[cfg(windows)]
impl Drop for HostJob {
    fn drop(&mut self) {
        // No inheritable or duplicated Job handle exists, so close terminates this host tree.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(unix)]
pub(super) fn signal_shell_session(session: i32, signal: i32) -> std::io::Result<()> {
    use std::collections::BTreeMap;
    if session <= 1 || session == unsafe { libc::getsid(0) } {
        return Err(std::io::Error::other(
            "Refusing to signal an unowned terminal session.",
        ));
    }
    // The supervisor keeps its actual shell Child unreaped throughout cleanup. Its new SID
    // therefore retains the launch identity while process groups are discovered and signalled.
    let mut groups = BTreeMap::new();
    for pid in process_ids()? {
        if pid > 1 && unsafe { libc::getsid(pid) } == session {
            let group = unsafe { libc::getpgid(pid) };
            if group > 1 {
                groups.insert(group, pid);
            }
        }
    }
    for (group, member) in groups {
        if unsafe { libc::getsid(member) } != session || unsafe { libc::getpgid(member) } != group {
            continue;
        }
        if unsafe { libc::kill(-group, signal) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn process_ids() -> std::io::Result<Vec<i32>> {
    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        {
            if pids.len() >= 131072 {
                return Err(std::io::Error::other(
                    "Process inventory exceeds its limit.",
                ));
            }
            pids.push(pid);
        }
    }
    Ok(pids)
}

#[cfg(target_os = "macos")]
fn process_ids() -> std::io::Result<Vec<i32>> {
    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_listallpids(buffer: *mut libc::c_void, buffersize: libc::c_int) -> libc::c_int;
    }
    let mut pids = vec![0_i32; 131072];
    let count = unsafe {
        proc_listallpids(
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        )
    };
    if count <= 0 || count as usize >= pids.len() {
        return Err(std::io::Error::other(
            "Could not enumerate the owned terminal session.",
        ));
    }
    pids.truncate(count as usize);
    Ok(pids)
}
