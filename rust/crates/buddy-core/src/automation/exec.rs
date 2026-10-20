//! Bounded pipe capture and cleanup of the captured worker's process family.
use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::journal::WorkerResult;
use super::models::{JobConfig, Shell};

const MAX_OUTPUT: usize = 512_000;
const CLEANUP: Duration = Duration::from_secs(5);

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub(super) command: String,
    pub(super) shell: Shell,
    pub(super) directory: PathBuf,
    pub(super) environment: BTreeMap<String, String>,
    timeout_ms: u64,
}

impl Spec {
    pub fn capture(config: &JobConfig) -> Result<Self, &'static str> {
        let command = config
            .command
            .clone()
            .ok_or("Specify a command for this job.")?;
        if command == "__copilot_snapshot__" {
            return Err("This system job requires the Copilot snapshot worker.");
        }
        let timeout = config.timeout.unwrap_or(30_000.0);
        if !timeout.is_finite() || timeout < 1.0 || timeout.fract() != 0.0 || timeout > 86_400_000.0
        {
            return Err("Choose a timeout from 1 to 86400000 milliseconds.");
        }
        let cwd = std::env::current_dir().map_err(|_| "The worker directory is unavailable.")?;
        let selected = config
            .cwd
            .as_deref()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or(cwd.clone());
        let directory = if selected.is_absolute() {
            selected
        } else {
            cwd.join(selected)
        };
        let environment = match config.extra.get("env") {
            None => BTreeMap::new(),
            Some(value) => value
                .as_object()
                .ok_or("Environment must be a JSON object of strings.")?
                .iter()
                .map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (key.clone(), value.to_owned()))
                        .ok_or("Environment values must be strings.")
                })
                .collect::<Result<_, _>>()?,
        };
        let spec = Self {
            command,
            shell: config.shell.unwrap_or(if cfg!(windows) {
                Shell::Powershell
            } else {
                Shell::Bash
            }),
            directory,
            environment,
            timeout_ms: timeout as u64,
        };
        spec.validate()?;
        Ok(spec)
    }

    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.command.is_empty() || self.command.len() > 64 * 1024 || self.command.contains('\0')
        {
            return Err("The command must contain 1 to 65536 bytes without NUL.");
        }
        if self.timeout_ms == 0 || self.timeout_ms > 86_400_000 {
            return Err("The worker timeout is unsupported.");
        }
        if !self.directory.is_absolute() || !self.directory.is_dir() {
            return Err("Choose an existing working directory.");
        }
        if self.environment.len() > 1024
            || self
                .environment
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
                > 64 * 1024
            || self.environment.iter().any(|(key, value)| {
                key.is_empty() || key.contains(['\0', '=']) || value.contains('\0')
            })
        {
            return Err("The explicit environment is invalid or exceeds 65536 bytes.");
        }
        Ok(())
    }

    pub(super) fn shell_command(&self) -> (&'static str, Vec<String>) {
        match self.shell {
            Shell::Powershell => (
                if cfg!(windows) { "pwsh.exe" } else { "pwsh" },
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    format!(
                        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {}",
                        self.command
                    ),
                ],
            ),
            Shell::Bash => ("bash", vec!["-c".into(), self.command.clone()]),
            Shell::Cmd => ("cmd.exe", vec!["/c".into(), self.command.clone()]),
        }
    }
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}
type Output = Arc<Mutex<Capture>>;

fn text(capture: &Capture) -> (String, bool) {
    let mut text = String::from_utf8_lossy(&capture.bytes).into_owned();
    let expanded = text.len() > MAX_OUTPUT;
    if expanded {
        let mut end = MAX_OUTPUT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    (text, capture.truncated || expanded)
}
fn capture(reader: impl Read + Send + 'static, output: Output) -> mpsc::Receiver<io::Result<()>> {
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut reader = reader;
        let result = (|| {
            let mut buffer = [0; 8192];
            loop {
                let count = match reader.read(&mut buffer) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    result => result?,
                };
                if count == 0 {
                    return Ok(());
                }
                let mut state = output.lock().unwrap_or_else(|error| error.into_inner());
                let retained = count.min(MAX_OUTPUT - state.bytes.len());
                state.bytes.extend_from_slice(&buffer[..retained]);
                state.truncated |= retained != count;
            }
        })();
        let _ = send.send(result);
    });
    receive
}

struct OwnedHost {
    child: Child,
    input: Option<ChildStdin>,
    #[cfg(windows)]
    job: Option<crate::terminal::ownership::HostJob>,
    reaped: bool,
}
impl OwnedHost {
    fn exited(&mut self) -> io::Result<Option<Option<i32>>> {
        #[cfg(unix)]
        {
            if !crate::terminal::ownership::shell_exited(self.child.id() as i32)? {
                return Ok(None);
            }
            // Keep the actual child unreaped until its session has been stopped.
            Ok(Some(None))
        }
        #[cfg(windows)]
        {
            match self.child.try_wait()? {
                None => Ok(None),
                Some(status) => {
                    self.reaped = true;
                    Ok(Some(status.code()))
                }
            }
        }
    }

    fn cleanup(&mut self) -> io::Result<Option<i32>> {
        #[cfg(unix)]
        let cleanup = if !self.reaped {
            crate::terminal::ownership::shell_exited(self.child.id() as i32)?;
            crate::terminal::ownership::signal_shell_session(self.child.id() as i32, libc::SIGKILL)
        } else {
            Ok(())
        };
        #[cfg(windows)]
        let cleanup = match &self.job {
            Some(job) => job.terminate_and_wait(CLEANUP),
            None => self.child.kill(),
        };
        self.input.take();
        // Even cleanup failure must not skip reaping our actual child.
        let status = if !self.reaped {
            let _ = self.child.kill();
            let status = self.child.wait()?;
            self.reaped = true;
            status.code()
        } else {
            None
        };
        cleanup?;
        Ok(status)
    }
}
impl Drop for OwnedHost {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.cleanup();
        }
    }
}

/// A handle can be dropped without blocking the UI; its supervisor still owns cleanup.
pub struct Session {
    cancel: Arc<AtomicBool>,
    stdout: Output,
    stderr: Output,
    result: Arc<Mutex<Option<WorkerResult>>>,
    supervisor: Option<std::thread::JoinHandle<()>>,
}
impl Session {
    pub fn start(executable: &Path, spec: Spec) -> Result<Self, &'static str> {
        let timeout_ms = spec.timeout_ms;
        Self::launch(
            executable,
            super::host::Start::Exec(Box::new(spec)),
            timeout_ms,
        )
    }

    pub fn start_ai(executable: &Path, spec: super::ai::Spec) -> Result<Self, &'static str> {
        let timeout_ms = spec.timeout_ms;
        Self::launch(
            executable,
            super::host::Start::Ai(Box::new(spec)),
            timeout_ms,
        )
    }

    pub fn start_snapshot(
        executable: &Path,
        spec: super::snapshot::Spec,
    ) -> Result<Self, &'static str> {
        let timeout_ms = spec.timeout_ms;
        Self::launch(
            executable,
            super::host::Start::Snapshot(Box::new(spec)),
            timeout_ms,
        )
    }

    fn launch(
        executable: &Path,
        start: super::host::Start,
        timeout_ms: u64,
    ) -> Result<Self, &'static str> {
        start.validate()?;
        if !executable.is_absolute() || !executable.is_file() {
            return Err("The trusted worker executable is unavailable.");
        }
        let mut command = Command::new(executable);
        command
            .arg("--automation-host")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
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
        let child = command
            .spawn()
            .map_err(|_| "The trusted worker host could not start.")?;
        let mut host = OwnedHost {
            child,
            input: None,
            #[cfg(windows)]
            job: None,
            reaped: false,
        };
        #[cfg(windows)]
        {
            host.job = Some(
                crate::terminal::ownership::HostJob::assign(&host.child)
                    .map_err(|_| "The worker's process ownership could not be established.")?,
            );
        }
        let mut input = host
            .child
            .stdin
            .take()
            .ok_or("The worker control pipe is unavailable.")?;
        super::host::send_start(&mut input, &start)
            .map_err(|_| "The worker start was interrupted.")?;
        host.input = Some(input);
        let stdout = Arc::new(Mutex::new(Capture::default()));
        let stderr = Arc::new(Mutex::new(Capture::default()));
        let out_done = capture(
            host.child
                .stdout
                .take()
                .ok_or("The worker output pipe is unavailable.")?,
            stdout.clone(),
        );
        let err_done = capture(
            host.child
                .stderr
                .take()
                .ok_or("The worker error pipe is unavailable.")?,
            stderr.clone(),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        let mut session = Self {
            cancel: cancel.clone(),
            stdout: stdout.clone(),
            stderr: stderr.clone(),
            result: result.clone(),
            supervisor: None,
        };
        let supervisor = std::thread::spawn(move || {
            let start = Instant::now();
            let timeout = Duration::from_millis(timeout_ms);
            let (mut exit_code, mut failure) = loop {
                if cancel.load(Ordering::Acquire) {
                    break (None, Some("Cancelled by user"));
                }
                if start.elapsed() >= timeout {
                    break (None, Some("The command exceeded its timeout"));
                }
                match host.exited() {
                    Ok(Some(code)) => break (code, None),
                    Err(_) => {
                        break (
                            None,
                            Some("The worker's process status could not be confirmed"),
                        );
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                }
            };
            match host.cleanup() {
                Ok(code) => {
                    if exit_code.is_none() {
                        exit_code = code;
                    }
                }
                Err(_) => {
                    failure.get_or_insert("The worker's process cleanup could not be confirmed");
                }
            }
            for done in [out_done, err_done] {
                if !matches!(done.recv_timeout(CLEANUP), Ok(Ok(()))) {
                    failure.get_or_insert("The worker's output cleanup could not be confirmed");
                }
            }
            let out = stdout.lock().unwrap_or_else(|error| error.into_inner());
            let err = stderr.lock().unwrap_or_else(|error| error.into_inner());
            let (stdout, stdout_truncated) = text(&out);
            let (stderr, stderr_truncated) = text(&err);
            let success = failure.is_none() && exit_code == Some(0);
            let finished = WorkerResult {
                success,
                output: Some(json!({"stdout":stdout,"stderr":stderr,
                    "stdoutTruncated":stdout_truncated,"stderrTruncated":stderr_truncated,"exitCode":exit_code})),
                error: failure.map(str::to_owned).or_else(|| {
                    (!success)
                        .then(|| format!("Process exited with code {}", exit_code.unwrap_or(-1)))
                }),
                duration: start.elapsed().as_secs_f64() * 1000.0,
            };
            *result.lock().unwrap_or_else(|error| error.into_inner()) = Some(finished);
        });
        session.supervisor = Some(supervisor);
        Ok(session)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn output(&self) -> (String, String) {
        let out = self
            .stdout
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let err = self
            .stderr
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (text(&out).0, text(&err).0)
    }
    pub fn result(&self) -> Option<WorkerResult> {
        self.result
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
    pub fn wait(mut self) -> Result<WorkerResult, &'static str> {
        if self
            .supervisor
            .take()
            .ok_or("The worker supervisor is unavailable.")?
            .join()
            .is_err()
        {
            return Err("The worker supervisor stopped unexpectedly.");
        }
        self.result().ok_or("The worker result is unavailable.")
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_utf8_cannot_expand_live_output_past_the_stream_limit() {
        let session = Session {
            cancel: Arc::new(AtomicBool::new(false)),
            stdout: Arc::new(Mutex::new(Capture {
                bytes: vec![0xff; MAX_OUTPUT],
                truncated: false,
            })),
            stderr: Arc::new(Mutex::new(Capture::default())),
            result: Arc::new(Mutex::new(None)),
            supervisor: None,
        };
        let (stdout, _) = session.output();
        assert!(
            stdout.len() <= MAX_OUTPUT,
            "decoded stream exceeded its byte limit"
        );
        assert!(!stdout.is_empty());
    }
}
