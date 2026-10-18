#[cfg(windows)]
pub(crate) struct HostJob(windows_sys::Win32::Foundation::HANDLE);

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
pub(super) fn shell_exited(pid: i32) -> std::io::Result<bool> {
    if pid <= 1 {
        return Err(std::io::Error::other("Invalid owned shell identity."));
    }
    // WNOWAIT observes our actual child without reaping it. Its reserved PID
    // must remain held until owned-session cleanup has finished.
    // https://man7.org/linux/man-pages/man2/waitpid.2.html
    // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_exit.c
    let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut information,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(error)
        };
    }
    Ok(unsafe { information.si_pid() } == pid)
}

#[cfg(any(unix, test))]
fn group_has_live_member(
    session: i32,
    group: i32,
    members: &[i32],
    mut identity: impl FnMut(i32) -> (i32, i32),
) -> bool {
    members
        .iter()
        .any(|member| identity(*member) == (session, group))
}

#[cfg(unix)]
pub(crate) fn signal_shell_session(session: i32, signal: i32) -> std::io::Result<()> {
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
                groups.entry(group).or_insert_with(Vec::new).push(pid);
            }
        }
    }
    for (group, members) in groups {
        if !group_has_live_member(session, group, &members, |member| {
            (unsafe { libc::getsid(member) }, unsafe {
                libc::getpgid(member)
            })
        }) {
            continue;
        }
        let result = if unsafe { libc::kill(-group, signal) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        };
        group_signal_result(result, || group_has_running_member(group))?;
    }
    Ok(())
}

#[cfg(unix)]
fn group_signal_result(
    result: std::io::Result<()>,
    has_running_member: impl FnOnce() -> std::io::Result<bool>,
) -> std::io::Result<()> {
    match result {
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        // Darwin returns EPERM for a group containing only unreaped zombies.
        // The last live member can exit after our ownership check. Confirm
        // absence with a fresh inventory; a live group or failed query stays red.
        Err(error) if error.raw_os_error() == Some(libc::EPERM) => {
            if has_running_member()? {
                Err(error)
            } else {
                Ok(())
            }
        }
        result => result,
    }
}

#[cfg(unix)]
fn group_has_running_member(group: i32) -> std::io::Result<bool> {
    for pid in process_ids()? {
        if pid <= 1 {
            continue;
        }
        let actual = unsafe { libc::getpgid(pid) };
        if actual == group {
            return Ok(true);
        }
        if actual < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod group_revalidation_tests {
    use super::group_has_live_member;

    #[test]
    fn a_vanished_representative_does_not_hide_a_live_owned_group_member() {
        // The last enumerated PID previously replaced the other group's PID.
        // It has now exited, while the earlier member still belongs to this session.
        assert!(group_has_live_member(100, 200, &[201, 202], |pid| {
            if pid == 201 { (100, 200) } else { (-1, -1) }
        }));
    }

    #[test]
    fn vanished_foreign_and_moved_members_never_authorize_signalling() {
        assert!(!group_has_live_member(100, 200, &[201, 202, 203], |pid| {
            match pid {
                201 => (-1, -1),
                202 => (999, 200),
                _ => (100, 999),
            }
        }));
        assert!(!group_has_live_member(100, 200, &[], |_| unreachable!()));
    }
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

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn permission_errors_require_a_fresh_empty_group_before_cleanup_succeeds() {
        let permission = || std::io::Error::from_raw_os_error(libc::EPERM);
        assert!(super::group_signal_result(Err(permission()), || Ok(false)).is_ok());
        assert_eq!(
            super::group_signal_result(Err(permission()), || Ok(true))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        assert_eq!(
            super::group_signal_result(Err(permission()), || {
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            })
            .unwrap_err()
            .raw_os_error(),
            Some(libc::EIO)
        );
    }

    #[test]
    fn a_group_that_exits_after_validation_is_already_stopped() {
        use std::io::Write;
        use std::os::unix::process::CommandExt;
        let mut command = std::process::Command::new("/bin/sh");
        command
            .args(["-c", "read value; exit 7"])
            .stdin(std::process::Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut child = command.spawn().unwrap();
        let pid = child.id() as i32;
        assert!(super::group_has_live_member(pid, pid, &[pid], |member| {
            (unsafe { libc::getsid(member) }, unsafe {
                libc::getpgid(member)
            })
        }));
        child.stdin.take().unwrap().write_all(b"q\n").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !super::shell_exited(pid).unwrap() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let result = if unsafe { libc::kill(-pid, libc::SIGKILL) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        };
        if let Err(error) = &result {
            assert_eq!(error.raw_os_error(), Some(libc::EPERM));
        }
        let cleanup = super::group_signal_result(result, || super::group_has_running_member(pid));
        assert_eq!(child.wait().unwrap().code(), Some(7));
        assert!(cleanup.is_ok(), "Exited owned session cleanup: {cleanup:?}");
    }

    #[test]
    fn exited_leader_cleanup_stops_its_live_descendant_and_preserves_an_unrelated_child() {
        use std::io::{BufRead, Write};
        use std::os::unix::process::CommandExt;
        let mut unrelated = std::process::Command::new("/bin/sh")
            .args(["-c", "read value"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut command = std::process::Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30 & echo $!; read value; exit 7"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut child = command.spawn().unwrap();
        let pid = child.id() as i32;
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let descendant: i32 = line.trim().parse().unwrap();
        assert_eq!(unsafe { libc::getsid(descendant) }, pid);
        child.stdin.take().unwrap().write_all(b"q\n").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !super::shell_exited(pid).unwrap() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let cleanup = super::signal_shell_session(pid, libc::SIGKILL);
        let status = child.wait().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while unsafe { libc::getsid(descendant) } == pid {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let unrelated_alive = unrelated.try_wait().unwrap().is_none();
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        assert_eq!(status.code(), Some(7));
        assert!(cleanup.is_ok(), "Exited owned session cleanup: {cleanup:?}");
        assert!(unrelated_alive);
    }

    #[test]
    fn observing_shell_exit_leaves_its_original_child_identity_waitable() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !super::shell_exited(pid).unwrap() {
            assert!(
                std::time::Instant::now() < deadline,
                "Owned child exit was not observed"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(super::shell_exited(pid).unwrap());
        assert_eq!(child.wait().unwrap().code(), Some(7));
        assert!(super::shell_exited(pid).is_err());
        assert!(super::shell_exited(1).is_err());
    }
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
    if count <= 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error().is_some_and(|code| code != 0) {
            return Err(error);
        }
        return Err(std::io::Error::other(
            "Could not enumerate the owned terminal session.",
        ));
    }
    if count as usize >= pids.len() {
        return Err(std::io::Error::other(
            "Could not enumerate the owned terminal session.",
        ));
    }
    pids.truncate(count as usize);
    Ok(pids)
}
