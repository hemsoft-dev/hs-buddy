//! Processes a run started that left its session. On macOS and Linux a run owns a session,
//! but a descendant that calls `setsid` (a daemonizing server, for example) leaves it, and
//! signalling the session no longer reaches it. Every process a run starts carries
//! [`RUN_VARIABLE`] in its environment, which descendants inherit wherever they move, so
//! the end of a run also ends every process of this user that still carries it.
use std::time::{Duration, Instant};

use super::host::RUN_VARIABLE;

/// How long processes that leave the session get to die before the run fails.
const DEADLINE: Duration = Duration::from_secs(5);

/// Ends every process of this user whose environment holds `RUN_VARIABLE=<run>`. An error
/// means one could not be ended, so it may still be changing the repository.
pub(super) fn end_marked(run: &str) -> std::io::Result<()> {
    let entry = format!("{RUN_VARIABLE}={run}").into_bytes();
    let own = unsafe { libc::getpid() };
    let marked = |pid: i32| {
        pid > 1
            && pid != own
            && environment(pid).is_some_and(|bytes| {
                bytes
                    .split(|byte| *byte == 0)
                    .any(|item| item == entry.as_slice())
            })
    };
    let deadline = Instant::now() + DEADLINE;
    loop {
        let found = crate::terminal::ownership::process_ids()?
            .into_iter()
            .filter(|pid| marked(*pid))
            .collect::<Vec<_>>();
        if found.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::other(
                "A process the loop started outside its session could not be ended.",
            ));
        }
        for pid in found {
            // Checked again just before the signal, so a reused ID is left alone.
            if marked(pid) {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The environment `pid` started with, as NUL-separated `NAME=value` items. A zombie or
/// another user's process has none.
#[cfg(target_os = "linux")]
fn environment(pid: i32) -> Option<Vec<u8>> {
    std::fs::read(format!("/proc/{pid}/environ"))
        .ok()
        .filter(|bytes| !bytes.is_empty())
}

/// `KERN_PROCARGS2` holds the argument count, the executable path, the arguments and then
/// the environment, all NUL-separated. Only `NAME=value` items are matched, so reading the
/// whole buffer finds the environment.
#[cfg(target_os = "macos")]
fn environment(pid: i32) -> Option<Vec<u8>> {
    let mut size: libc::size_t = 0;
    let mut name = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: a size query with a null buffer; `name` and `size` outlive the call.
    let queried = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            name.len() as libc::c_uint,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if queried != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0_u8; size];
    // SAFETY: `buffer` holds `size` bytes, which the kernel never exceeds.
    let read = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            name.len() as libc::c_uint,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buffer.truncate(size);
    Some(buffer)
}
