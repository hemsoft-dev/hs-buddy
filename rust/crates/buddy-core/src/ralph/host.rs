//! The detached host that owns one loop, and the exec process that starts PowerShell.
//!
//! ```text
//! app ──spawn──▶ --ralph-host <run>   no console, own session; outlives the app
//!                 └─▶ --ralph-exec    Job object (Windows) / session leader (Unix)
//!                       └─▶ pwsh -File <script> …  and everything it starts
//! ```
//!
//! The host locks `host.lock` for its lifetime and is the only writer of the run's status.
//! A stop request, or the end of the loop, terminates exactly the exec process's Job or
//! session. If the host itself dies, its Job handle closes (Windows) or the exec process
//! sees its control pipe close and ends its own session (Unix), so no loop runs unwatched.
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::launch::Plan;
use super::progress::{Progress, strip_ansi};
use super::store::{
    HostState, LOCK, OUTPUT, REQUEST, Request, STARTED, STATUS, STOP, Status, now_ms, private_file,
    write_atomically, write_private,
};

const MAX_START: usize = 1024 * 1024;
/// The generated script PowerShell runs; see [`Plan::launcher`].
const LAUNCHER: &str = "launch.ps1";
const MAX_LINE: usize = 64 * 1024;
const MAX_LOG: u64 = 64 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(100);
const STATUS_INTERVAL: Duration = Duration::from_millis(500);
const CLEANUP: Duration = Duration::from_secs(5);

/// Starts the host for the run in `folder` and returns without waiting for it.
#[cfg(unix)]
pub fn spawn_host(executable: &Path, folder: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(executable);
    command
        .arg("--ralph-host")
        .arg(folder)
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Its own session, so closing the app's terminal or process group leaves it running.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() >= 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("The loop host could not start: {error}"))?;
    // Reap it whenever it ends; the run's files carry its result.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// `"executable" --ralph-host "folder"`. Neither path can contain a quote on Windows,
/// so only backslashes before the closing quote need doubling.
#[cfg(any(windows, test))]
fn host_command_line(executable: &Path, folder: &Path) -> Result<Vec<u16>, String> {
    let quote = |path: &Path| -> Result<String, String> {
        let text = path
            .to_str()
            .ok_or("The run folder path is not valid Unicode.")?;
        if text.contains('"') || text.contains('\0') {
            return Err("The run folder path is not supported.".into());
        }
        let trailing = text.len() - text.trim_end_matches('\\').len();
        Ok(format!("\"{text}{}\"", "\\".repeat(trailing)))
    };
    let line = format!("{} --ralph-host {}", quote(executable)?, quote(folder)?);
    Ok(line.encode_utf16().chain(Some(0)).collect())
}

/// Starts the host for the run in `folder` and returns without waiting for it.
///
/// `CreateProcessW` without handle inheritance: std's spawn passes on every inheritable
/// handle the app holds, such as pipes from whatever started Buddy, and a host that
/// outlives the app would keep those open.
#[cfg(windows)]
pub fn spawn_host(executable: &Path, folder: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW,
        PROCESS_INFORMATION, STARTUPINFOW,
    };
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let (application, directory) = (wide(executable), wide(folder));
    let mut last = 0;
    // A Job that forbids breaking away refuses the first form; start inside it then.
    for flags in [
        CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
        CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
    ] {
        let mut line = host_command_line(executable, folder)?;
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                flags,
                std::ptr::null(),
                directory.as_ptr(),
                &startup,
                &mut process,
            )
        };
        if created != 0 {
            unsafe {
                CloseHandle(process.hThread);
                CloseHandle(process.hProcess);
            }
            return Ok(());
        }
        last = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if last != 5 {
            break;
        }
    }
    Err(format!(
        "The loop host could not start: {}",
        io::Error::from_raw_os_error(last)
    ))
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Start {
    program: PathBuf,
    arguments: Vec<OsString>,
    directory: PathBuf,
    path: OsString,
}

fn send_start(writer: &mut impl Write, start: &Start) -> io::Result<()> {
    let bytes = serde_json::to_vec(start).map_err(io::Error::other)?;
    if bytes.len() > MAX_START {
        return Err(io::Error::other(
            "The loop start request exceeds its limit.",
        ));
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

fn receive_start(reader: &mut impl Read) -> io::Result<Start> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_le_bytes(header) as usize;
    if size == 0 || size > MAX_START {
        return Err(io::Error::other("Unsupported loop start packet."));
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes)?;
    let start: Start = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::other("Unsupported loop start packet."))?;
    if !start.program.is_absolute() || !start.directory.is_absolute() {
        return Err(io::Error::other("Unsupported loop start packet."));
    }
    Ok(start)
}

/// `--ralph-exec`: reads one start packet, runs it, and returns its exit code.
pub fn run_ralph_exec() -> i32 {
    let mut input = io::stdin();
    let start = match receive_start(&mut input) {
        Ok(start) => start,
        Err(_) => {
            let _ = writeln!(io::stderr(), "[Buddy] The loop start request was invalid.");
            return 125;
        }
    };
    parent_death(input);
    let mut command = Command::new(&start.program);
    command
        .args(&start.arguments)
        .current_dir(&start.directory)
        .env("PATH", &start.path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match command.spawn() {
        Ok(mut child) => child
            .wait()
            .ok()
            .and_then(|status| status.code())
            .unwrap_or(1),
        Err(error) => {
            let _ = writeln!(
                io::stderr(),
                "[Buddy] PowerShell could not start ({}): {error}",
                start.program.display()
            );
            127
        }
    }
}

fn parent_death(mut input: io::Stdin) {
    std::thread::spawn(move || {
        let mut byte = [0; 1];
        // EOF means the host is gone.
        let _ = input.read(&mut byte);
        #[cfg(unix)]
        {
            let _ = crate::terminal::ownership::terminate_own_host_session();
        }
        // On Windows the host's Job closes with it and takes this process tree along.
        std::process::exit(125);
    });
}

/// `--ralph-host <run folder>`: owns one run from start to finish.
pub fn run_ralph_host(folder: &Path) -> i32 {
    let Ok(lock) = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(folder.join(LOCK))
    else {
        return 2;
    };
    if lock.try_lock().is_err() {
        // Another host owns this run.
        return 3;
    }
    if OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join(STARTED))
        .is_err()
    {
        // This run started before; it is never started again.
        return 4;
    }
    let mut host = Host::new(folder);
    let request = std::fs::read(folder.join(REQUEST))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Request>(&bytes).ok())
        .filter(|request| {
            folder
                .file_name()
                .is_some_and(|name| name == request.id.as_str())
        });
    match request {
        Some(request) => host.run(&request.plan),
        None => host.finish(
            HostState::Failed,
            None,
            Some("The run request could not be read.".into()),
        ),
    }
    drop(lock);
    0
}

struct Log {
    file: Option<BufWriter<File>>,
    written: u64,
    capped: bool,
    pending: bool,
    flushed: Instant,
}

impl Log {
    fn open(path: &Path) -> Self {
        let file = private_file().create(true).append(true).open(path).ok();
        let written = file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map_or(0, |meta| meta.len());
        Self {
            file: file.map(BufWriter::new),
            written,
            capped: false,
            pending: false,
            flushed: Instant::now(),
        }
    }

    fn line(&mut self, text: &str) {
        let Some(file) = &mut self.file else {
            return;
        };
        if self.capped {
            return;
        }
        let size = text.len() as u64 + 1;
        if self.written + size > MAX_LOG {
            self.capped = true;
            let _ = writeln!(
                file,
                "[Buddy] Output beyond 64 MB is not saved. The loop keeps running."
            );
            return;
        }
        if writeln!(file, "{text}").is_ok() {
            self.written += size;
            self.pending = true;
        }
        self.tick();
    }

    /// Flushes buffered lines at most every 200 ms, and a quiet loop's last lines too.
    fn tick(&mut self) {
        if self.pending && self.flushed.elapsed() >= Duration::from_millis(200) {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if let Some(file) = &mut self.file {
            let _ = file.flush();
        }
        self.pending = false;
        self.flushed = Instant::now();
    }
}

enum Outcome {
    Exited(i32),
    Stopped,
    Error(String),
}

struct Host {
    folder: PathBuf,
    status: Status,
    log: Log,
    written: Instant,
    dirty: bool,
}

impl Host {
    fn new(folder: &Path) -> Self {
        let now = now_ms();
        let mut host = Self {
            folder: folder.to_path_buf(),
            status: Status {
                state: HostState::Running,
                host_pid: std::process::id(),
                cycle: 0,
                progress: Progress::default(),
                exit_code: None,
                error: None,
                started_at: now,
                updated_at: now,
                finished_at: None,
            },
            log: Log::open(&folder.join(OUTPUT)),
            written: Instant::now(),
            dirty: false,
        };
        host.write_status();
        host
    }

    fn stop_requested(&self) -> bool {
        self.folder.join(STOP).exists()
    }

    fn write_status(&mut self) {
        self.status.updated_at = now_ms();
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.status) {
            let _ = write_atomically(&self.folder.join(STATUS), &bytes);
        }
        self.written = Instant::now();
        self.dirty = false;
    }

    fn finish(&mut self, state: HostState, exit_code: Option<i32>, error: Option<String>) {
        match &error {
            Some(error) => self.log.line(&format!("[Buddy] {error}")),
            None if state == HostState::Stopped => self.log.line("[Buddy] Stopped."),
            None => {}
        }
        self.log.flush();
        self.status.state = state;
        self.status.exit_code = exit_code;
        self.status.error = error;
        self.status.finished_at = Some(now_ms());
        self.write_status();
    }

    fn run(&mut self, plan: &Plan) {
        let cycles = plan.cycles.max(1);
        for cycle in 1..=cycles {
            if self.stop_requested() {
                return self.finish(HostState::Stopped, None, None);
            }
            if cycle > 1
                && let Err(error) = self.refresh(plan)
            {
                return self.finish(HostState::Failed, None, Some(error));
            }
            self.status.cycle = cycle;
            if cycles > 1 {
                self.log
                    .line(&format!("[Buddy] Cycle {cycle} of {cycles} started."));
            }
            self.write_status();
            match self.cycle(plan) {
                Outcome::Exited(0) => {}
                Outcome::Exited(code) => {
                    return self.finish(
                        HostState::Failed,
                        Some(code),
                        Some(format!("The loop exited with code {code}.")),
                    );
                }
                Outcome::Stopped => return self.finish(HostState::Stopped, None, None),
                Outcome::Error(error) => return self.finish(HostState::Failed, None, Some(error)),
            }
        }
        self.finish(HostState::Completed, Some(0), None)
    }

    /// Before another cycle, fast-forward the repository's checked-out branch from its
    /// upstream, as `ralph-repeat.ps1` did, so the next cycle starts from merged work.
    /// Unlike that wrapper, the checkout is never switched to another branch.
    fn refresh(&mut self, plan: &Plan) -> Result<(), String> {
        let upstream = self.git(plan, &["rev-parse", "--abbrev-ref", "@{upstream}"])?;
        if !upstream.status.success() {
            self.log.line(
                "[Buddy] The checked-out branch has no upstream; the next cycle starts from the local checkout.",
            );
            return Ok(());
        }
        let name = String::from_utf8_lossy(&upstream.stdout).trim().to_owned();
        self.log.line(&format!(
            "[Buddy] Fast-forwarding from {name} before the next cycle."
        ));
        let pull = self.git(plan, &["pull", "--ff-only"])?;
        for bytes in [&pull.stdout, &pull.stderr] {
            for line in String::from_utf8_lossy(bytes).lines() {
                if !line.trim().is_empty() {
                    self.log.line(&format!("[git] {}", strip_ansi(line)));
                }
            }
        }
        if pull.status.success() {
            Ok(())
        } else {
            Err("The repository could not be fast-forwarded before the next cycle, so the loop stopped. Update the checkout, then retry.".into())
        }
    }

    /// Runs Git in the repository with the loop's PATH, giving up after five minutes.
    fn git(&self, plan: &Plan, arguments: &[&str]) -> Result<std::process::Output, String> {
        let mut command = Command::new("git");
        command
            .args(arguments)
            .current_dir(&plan.directory)
            .env("PATH", &plan.path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("Git could not run before the next cycle: {error}"))?;
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(
                        "Git did not finish updating the repository before the next cycle.".into(),
                    );
                }
            }
        }
        child
            .wait_with_output()
            .map_err(|error| format!("Git's output could not be read: {error}"))
    }

    fn output(&mut self, error: bool, bytes: &[u8]) {
        let line = strip_ansi(&String::from_utf8_lossy(bytes));
        if line.trim().is_empty() {
            return;
        }
        if !error && self.status.progress.observe(&line) {
            self.dirty = true;
        }
        if error {
            self.log.line(&format!("[stderr] {line}"));
        } else {
            self.log.line(&line);
        }
    }

    fn cycle(&mut self, plan: &Plan) -> Outcome {
        let mut owned = match Owned::spawn() {
            Ok(owned) => owned,
            Err(error) => return Outcome::Error(error),
        };
        let launcher = self.folder.join(LAUNCHER);
        if let Err(error) = write_private(&launcher, plan.launcher().as_bytes()) {
            let _ = owned.cleanup();
            return Outcome::Error(format!("The loop launcher could not be written: {error}"));
        }
        let start = Start {
            program: plan.program.clone(),
            arguments: plan.command_line(&launcher),
            directory: plan.directory.clone(),
            path: plan.path.clone(),
        };
        let (sender, receiver) = mpsc::channel();
        if let Some(stdout) = owned.child.stdout.take() {
            read_lines(Box::new(stdout), false, sender.clone());
        }
        if let Some(stderr) = owned.child.stderr.take() {
            read_lines(Box::new(stderr), true, sender);
        }
        if owned
            .input
            .as_mut()
            .is_none_or(|input| send_start(input, &start).is_err())
        {
            let _ = owned.cleanup();
            return Outcome::Error("The loop could not be started.".into());
        }
        let mut stopped = false;
        let mut checked = Instant::now();
        let exited = loop {
            match receiver.recv_timeout(POLL) {
                Ok((error, line)) => self.output(error, &line),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                // Both streams closed; the process may still be running.
                Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(POLL),
            }
            self.log.tick();
            if self.dirty && self.written.elapsed() >= STATUS_INTERVAL {
                self.write_status();
            }
            // Checked on a clock, so steady output cannot delay a stop.
            if checked.elapsed() < POLL {
                continue;
            }
            checked = Instant::now();
            if self.stop_requested() {
                stopped = true;
                break Ok(None);
            }
            match owned.exited() {
                Ok(Some(code)) => break Ok(code),
                Ok(None) => {}
                Err(error) => break Err(error),
            }
        };
        let cleaned = owned.cleanup();
        // Writers are gone once the process tree is; drain what they left.
        let deadline = Instant::now() + CLEANUP;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match receiver.recv_timeout(left) {
                Ok((error, line)) => self.output(error, &line),
                Err(_) => break,
            }
        }
        if self.dirty {
            self.write_status();
        }
        match (stopped, exited, cleaned) {
            (true, _, _) => Outcome::Stopped,
            (_, Err(_), _) => {
                Outcome::Error("The loop's process status could not be confirmed.".into())
            }
            (_, Ok(Some(code)), _) => Outcome::Exited(code),
            (_, Ok(None), Ok(Some(code))) => Outcome::Exited(code),
            (_, Ok(None), _) => Outcome::Error("The loop ended without an exit code.".into()),
        }
    }
}

/// Splits a stream at `\n` or a bare `\r`, in pieces of at most [`MAX_LINE`] bytes.
fn read_lines(
    mut reader: Box<dyn Read + Send>,
    error: bool,
    sender: mpsc::Sender<(bool, Vec<u8>)>,
) {
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        let mut line = Vec::new();
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            for &byte in &buffer[..count] {
                if byte == b'\n' || byte == b'\r' {
                    if sender.send((error, std::mem::take(&mut line))).is_err() {
                        return;
                    }
                } else {
                    line.push(byte);
                    if line.len() >= MAX_LINE
                        && sender.send((error, std::mem::take(&mut line))).is_err()
                    {
                        return;
                    }
                }
            }
        }
        if !line.is_empty() {
            let _ = sender.send((error, line));
        }
    });
}

/// The exec process and everything it starts.
struct Owned {
    child: Child,
    input: Option<std::process::ChildStdin>,
    #[cfg(windows)]
    job: Option<crate::terminal::ownership::HostJob>,
    reaped: Option<Option<i32>>,
}

impl Owned {
    fn spawn() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|_| "The loop host executable is unavailable.".to_string())?;
        let mut command = Command::new(executable);
        command
            .arg("--ralph-exec")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() >= 0 {
                        Ok(())
                    } else {
                        Err(io::Error::last_os_error())
                    }
                });
            }
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("The loop process could not start: {error}"))?;
        let input = child.stdin.take();
        // The exec process waits for its start packet, so it has started nothing yet.
        #[cfg(windows)]
        let job = match crate::terminal::ownership::HostJob::assign(&child) {
            Ok(job) => Some(job),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("The loop's process ownership could not be established.".into());
            }
        };
        Ok(Self {
            child,
            input,
            #[cfg(windows)]
            job,
            reaped: None,
        })
    }

    /// `Some(code)` once the exec process has ended, without releasing its identity on Unix.
    fn exited(&mut self) -> io::Result<Option<Option<i32>>> {
        #[cfg(unix)]
        {
            if crate::terminal::ownership::shell_exited(self.child.id() as i32)? {
                Ok(Some(None))
            } else {
                Ok(None)
            }
        }
        #[cfg(windows)]
        {
            Ok(self.child.try_wait()?.map(|status| {
                self.reaped = Some(status.code());
                status.code()
            }))
        }
    }

    /// Terminates whatever is left of the owned tree, then reaps the exec process.
    fn cleanup(&mut self) -> io::Result<Option<i32>> {
        #[cfg(windows)]
        let cleanup = match &self.job {
            Some(job) => job.terminate_and_wait(CLEANUP),
            None => self.child.kill(),
        };
        #[cfg(unix)]
        let cleanup = if self.reaped.is_none() {
            crate::terminal::ownership::signal_shell_session(self.child.id() as i32, libc::SIGKILL)
        } else {
            Ok(())
        };
        self.input.take();
        let code = match self.reaped {
            Some(code) => code,
            None => {
                let _ = self.child.kill();
                let code = self.child.wait()?.code();
                self.reaped = Some(code);
                code
            }
        };
        cleanup?;
        Ok(code)
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if self.reaped.is_none() {
            let _ = self.cleanup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_packets_round_trip_and_reject_relative_or_oversized_input() {
        let directory = std::env::temp_dir().join("ralph repo 界");
        let start = Start {
            program: std::env::temp_dir().join("pwsh"),
            arguments: vec!["-File".into(), "a b.ps1".into()],
            directory,
            path: "x".into(),
        };
        let mut bytes = vec![];
        send_start(&mut bytes, &start).unwrap();
        assert_eq!(receive_start(&mut bytes.as_slice()).unwrap(), start);
        let relative = Start {
            program: "pwsh".into(),
            ..start
        };
        let mut bytes = vec![];
        send_start(&mut bytes, &relative).unwrap();
        assert!(receive_start(&mut bytes.as_slice()).is_err());
        assert!(receive_start(&mut ((MAX_START as u32 + 1).to_le_bytes()).as_slice()).is_err());
        assert!(receive_start(&mut [0u8, 0, 0, 0].as_slice()).is_err());
    }

    #[test]
    fn host_command_lines_quote_paths_with_spaces() {
        let line = host_command_line(
            Path::new(r"C:\Program Files\Buddy\buddy.exe"),
            Path::new(r"C:\Users\Me\runs\a b\"),
        )
        .unwrap();
        assert_eq!(
            String::from_utf16(&line[..line.len() - 1]).unwrap(),
            r#""C:\Program Files\Buddy\buddy.exe" --ralph-host "C:\Users\Me\runs\a b\\""#
        );
        assert!(host_command_line(Path::new("a\"b"), Path::new("c")).is_err());
    }

    #[test]
    fn lines_split_at_either_line_ending_and_long_lines_in_pieces() {
        let (sender, receiver) = mpsc::channel();
        let mut input = b"one\r\ntwo\rthree\n".to_vec();
        input.extend(vec![b'x'; MAX_LINE + 3]);
        read_lines(Box::new(std::io::Cursor::new(input)), true, sender);
        let lines = receiver
            .iter()
            .map(|(error, line)| {
                assert!(error);
                line
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lines.iter().map(Vec::len).collect::<Vec<_>>(),
            [3, 0, 3, 5, MAX_LINE, 3]
        );
        assert_eq!(lines[0], b"one");
    }

    #[test]
    fn the_log_stops_at_its_limit_with_one_note() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(OUTPUT);
        let mut log = Log::open(&path);
        log.written = MAX_LOG - 4;
        log.line("abc");
        log.line("more text");
        log.line("ignored");
        log.flush();
        assert_eq!(
            std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n"),
            "abc\n[Buddy] Output beyond 64 MB is not saved. The loop keeps running.\n"
        );
    }

    #[test]
    fn a_host_never_starts_a_run_twice_or_while_another_host_holds_it() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join(STARTED), b"").unwrap();
        assert_eq!(run_ralph_host(folder.path()), 4);
        assert!(!folder.path().join(STATUS).exists());
        let other = tempfile::tempdir().unwrap();
        let holder = File::create(other.path().join(LOCK)).unwrap();
        holder.lock().unwrap();
        assert_eq!(run_ralph_host(other.path()), 3);
        assert!(!other.path().join(STARTED).exists());
    }

    #[test]
    fn an_unreadable_request_is_recorded_as_failed() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(run_ralph_host(folder.path()), 0);
        let status: Status =
            serde_json::from_slice(&std::fs::read(folder.path().join(STATUS)).unwrap()).unwrap();
        assert_eq!(status.state, HostState::Failed);
        assert_eq!(
            status.error.as_deref(),
            Some("The run request could not be read.")
        );
        assert!(!super::super::store::locked(&folder.path().join(LOCK)));
    }
}
