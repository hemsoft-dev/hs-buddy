//! The helper extracts the SDK's verified bundle without ambient authentication.
//! It cannot start until its parent has established process-tree ownership.
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use github_copilot_sdk::{CliProgram, Client};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::copilot_sdk::{client_options, private_directory};

pub const RUNTIME_VERSION: &str = "1.0.99-2";
const READY: &[u8] = b"prepare\n";
#[cfg(windows)]
const PROGRAM: &str = "copilot-runtime.exe";
#[cfg(not(windows))]
const PROGRAM: &str = "copilot-runtime";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallError {
    InvalidRoot,
    Unavailable,
    Cancelled,
    CleanupFailed {
        cancelled: bool,
        failure: CleanupFailure,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupStage {
    Ownership,
    Reap,
}

/// Safe diagnostics deliberately discard error messages, paths, and arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupFailure {
    pub stage: CleanupStage,
    pub kind: std::io::ErrorKind,
    pub os_code: Option<i32>,
}
impl CleanupFailure {
    fn capture(stage: CleanupStage, error: std::io::Error) -> Self {
        Self {
            stage,
            kind: error.kind(),
            os_code: error.raw_os_error(),
        }
    }
}
impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRoot => "The private assistant runtime directory is unavailable.",
            Self::Unavailable => "The packaged assistant runtime could not be prepared.",
            Self::Cancelled => "Assistant preparation was cancelled.",
            Self::CleanupFailed {
                cancelled: true, ..
            } => "Assistant preparation was cancelled. Process cleanup could not be confirmed.",
            Self::CleanupFailed {
                cancelled: false, ..
            } => "Assistant preparation failed. Process cleanup could not be confirmed.",
        })
    }
}
impl std::error::Error for InstallError {}

pub struct VerifiedRuntime {
    program: PathBuf,
}
impl VerifiedRuntime {
    pub fn program(&self) -> &Path {
        &self.program
    }
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    version: String,
    program: PathBuf,
    wrapper_sha256: String,
    runtime_sha256: String,
}

fn sensitive_name(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_uppercase();
    [
        "GH_",
        "GITHUB_",
        "COPILOT_",
        "OPENAI_",
        "ANTHROPIC_",
        "AZURE_",
        "CODEX_",
        "OTEL_",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
        || ["NODE_OPTIONS", "NODE_PATH", "DEBUG", "RUST_LOG"].contains(&name.as_str())
}

fn helper_command(executable: &Path, root: &Path) -> Result<Command, InstallError> {
    if !root.is_absolute() || !executable.is_absolute() || !executable.is_file() {
        return Err(InstallError::InvalidRoot);
    }
    private_directory(root).map_err(|_| InstallError::InvalidRoot)?;
    let root = root.canonicalize().map_err(|_| InstallError::InvalidRoot)?;
    let mut command = Command::new(executable);
    command
        .arg("--prepare-copilot-runtime")
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, _) in std::env::vars_os() {
        if sensitive_name(&name) {
            command.env_remove(name);
        }
    }
    for (key, directory) in [
        ("HOME", "home"),
        ("USERPROFILE", "home"),
        ("APPDATA", "appdata"),
        ("LOCALAPPDATA", "local-appdata"),
        ("XDG_CONFIG_HOME", "xdg-config"),
        ("XDG_CACHE_HOME", "xdg-cache"),
        ("GH_CONFIG_DIR", "gh"),
    ] {
        let directory = root.join("helper-environment").join(directory);
        private_directory(&directory).map_err(|_| InstallError::InvalidRoot)?;
        command.env(key, directory);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The child has no other threads before exec. All SDK children inherit this SID.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    Ok(command)
}

struct Owner {
    active: bool,
    #[cfg(windows)]
    job: crate::terminal::ownership::HostJob,
    #[cfg(unix)]
    session: i32,
}
impl Owner {
    fn attach(child: &Child) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            Ok(Self {
                active: true,
                job: crate::terminal::ownership::HostJob::assign(child)?,
            })
        }
        #[cfg(unix)]
        {
            Ok(Self {
                active: true,
                session: i32::try_from(child.id()).map_err(std::io::Error::other)?,
            })
        }
    }
    fn terminate(&self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            self.job.terminate_and_wait(Duration::from_secs(3))
        }
        #[cfg(unix)]
        {
            crate::terminal::ownership::signal_shell_session(self.session, libc::SIGKILL)
        }
    }
    fn finish(&mut self) -> std::io::Result<()> {
        let result = self.terminate();
        self.active = false;
        result
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if self.active {
            let _ = self.terminate();
        }
    }
}

fn hash_file(path: &Path) -> Result<String, InstallError> {
    let mut input = std::fs::File::open(path).map_err(|_| InstallError::Unavailable)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0usize;
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|_| InstallError::Unavailable)?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count);
        if total > 512 * 1024 * 1024 {
            return Err(InstallError::Unavailable);
        }
        digest.update(&buffer[..count]);
    }
    if total == 0 {
        return Err(InstallError::Unavailable);
    }
    Ok(crate::sha256::hex(&digest.finalize()))
}

fn verify_receipt(root: &Path, receipt: Receipt) -> Result<VerifiedRuntime, InstallError> {
    if receipt.version != RUNTIME_VERSION {
        return Err(InstallError::Unavailable);
    }
    let root = root.canonicalize().map_err(|_| InstallError::Unavailable)?;
    let program = receipt
        .program
        .canonicalize()
        .map_err(|_| InstallError::Unavailable)?;
    if !program.starts_with(&root) || program.file_name() != Some(std::ffi::OsStr::new(PROGRAM)) {
        return Err(InstallError::Unavailable);
    }
    let runtime = program
        .parent()
        .ok_or(InstallError::Unavailable)?
        .join("runtime.node")
        .canonicalize()
        .map_err(|_| InstallError::Unavailable)?;
    if !runtime.starts_with(&root) {
        return Err(InstallError::Unavailable);
    }
    if hash_file(&program)? != receipt.wrapper_sha256
        || hash_file(&runtime)? != receipt.runtime_sha256
    {
        return Err(InstallError::Unavailable);
    }
    Ok(VerifiedRuntime { program })
}

fn prepare_blocking(
    executable: &Path,
    root: &Path,
    cancelled: &AtomicBool,
) -> Result<VerifiedRuntime, InstallError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(InstallError::Cancelled);
    }
    let mut child = helper_command(executable, root)?
        .spawn()
        .map_err(|_| InstallError::Unavailable)?;
    let mut owner = match Owner::attach(&child) {
        Ok(owner) => owner,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(InstallError::Unavailable);
        }
    };
    let Some(mut input) = child.stdin.take() else {
        let _ = owner.finish();
        let _ = child.kill();
        let _ = child.wait();
        return Err(InstallError::Unavailable);
    };
    if input.write_all(READY).is_err() {
        let _ = owner.finish();
        let _ = child.kill();
        let _ = child.wait();
        return Err(InstallError::Unavailable);
    }
    let Some(stdout) = child.stdout.take() else {
        let _ = owner.finish();
        let _ = child.kill();
        let _ = child.wait();
        return Err(InstallError::Unavailable);
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::Builder::new()
        .name("buddy-runtime-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let read = BufReader::new(stdout.take(16 * 1024 + 1)).read_until(b'\n', &mut bytes);
            let _ = tx.send(read.map(|_| bytes));
        });
    let reader = match reader {
        Ok(reader) => reader,
        Err(_) => {
            let _ = owner.finish();
            let _ = child.kill();
            let _ = child.wait();
            return Err(InstallError::Unavailable);
        }
    };
    let deadline = Instant::now() + Duration::from_secs(95);
    let result = loop {
        if cancelled.load(Ordering::Acquire) {
            break Err(InstallError::Cancelled);
        }
        if Instant::now() >= deadline {
            break Err(InstallError::Unavailable);
        }
        match rx.try_recv() {
            Ok(Ok(bytes)) => break Ok(bytes),
            Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                break Err(InstallError::Unavailable);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(25))
            }
        }
    };
    // Never reap the helper before signalling its SID. Its unreaped PID cannot
    // be reused as an unrelated session, even if it exits before the receipt.
    let _ = input.write_all(b"q");
    let cleanup = owner
        .finish()
        .map_err(|error| CleanupFailure::capture(CleanupStage::Ownership, error));
    let _ = child.kill();
    let reaped = child
        .wait()
        .map(|_| ())
        .map_err(|error| CleanupFailure::capture(CleanupStage::Reap, error));
    let _ = reader.join();
    let bytes = preparation_result(result, cleanup.and(reaped))?;
    if bytes.len() > 16 * 1024 {
        return Err(InstallError::Unavailable);
    }
    let mut stderr = [0u8; 1];
    if child
        .stderr
        .take()
        .ok_or(InstallError::Unavailable)?
        .read(&mut stderr)
        .map_err(|_| InstallError::Unavailable)?
        != 0
    {
        return Err(InstallError::Unavailable);
    }
    verify_receipt(
        root,
        serde_json::from_slice(&bytes).map_err(|_| InstallError::Unavailable)?,
    )
}

fn preparation_result(
    primary: Result<Vec<u8>, InstallError>,
    cleanup: Result<(), CleanupFailure>,
) -> Result<Vec<u8>, InstallError> {
    if let Err(failure) = cleanup {
        return Err(InstallError::CleanupFailed {
            cancelled: matches!(primary, Err(InstallError::Cancelled)),
            failure,
        });
    }
    primary
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub async fn prepare(executable: PathBuf, root: PathBuf) -> Result<VerifiedRuntime, InstallError> {
    let cancelled = Arc::new(AtomicBool::new(false));
    prepare_with_cancel(executable, root, cancelled).await
}

pub(crate) async fn prepare_with_cancel(
    executable: PathBuf,
    root: PathBuf,
    cancelled: Arc<AtomicBool>,
) -> Result<VerifiedRuntime, InstallError> {
    let _cancel = CancelOnDrop(cancelled.clone());
    tokio::task::spawn_blocking(move || prepare_blocking(&executable, &root, &cancelled))
        .await
        .map_err(|_| InstallError::Unavailable)?
}

fn locate_runtime(root: &Path) -> Result<PathBuf, InstallError> {
    let embedded = root.join("embedded");
    let mut directories = vec![embedded.clone()];
    for child in std::fs::read_dir(&embedded)
        .map_err(|_| InstallError::Unavailable)?
        .take(16)
    {
        let child = child.map_err(|_| InstallError::Unavailable)?;
        if child
            .file_type()
            .map_err(|_| InstallError::Unavailable)?
            .is_dir()
        {
            directories.push(child.path());
        }
    }
    let programs: Vec<_> = directories
        .into_iter()
        .filter_map(|directory| {
            let program = directory.join(PROGRAM);
            let mut marker = String::new();
            std::fs::File::open(directory.join(".copilot-runtime-version"))
                .ok()?
                .take(65)
                .read_to_string(&mut marker)
                .ok()?;
            if marker.len() > 64 {
                return None;
            }
            (marker.trim() == RUNTIME_VERSION && program.is_file()).then_some(program)
        })
        .collect();
    if programs.len() != 1 {
        return Err(InstallError::Unavailable);
    }
    Ok(programs.into_iter().next().expect("one verified candidate"))
}

async fn install(root: &Path) -> Result<Receipt, InstallError> {
    if !root.is_absolute() {
        return Err(InstallError::InvalidRoot);
    }
    let expected_gh = root.join("helper-environment/gh");
    if std::env::var_os("GH_CONFIG_DIR")
        .map(PathBuf::from)
        .as_ref()
        != Some(&expected_gh)
        || std::env::vars_os().any(|(name, _)| {
            sensitive_name(&name) && name.to_string_lossy().to_ascii_uppercase() != "GH_CONFIG_DIR"
        })
    {
        return Err(InstallError::InvalidRoot);
    }
    let executable = std::env::current_exe().map_err(|_| InstallError::Unavailable)?;
    let mut options = client_options(
        &executable,
        &root.join("extraction-state"),
        None,
        std::env::vars_os().map(|(name, _)| name),
    )
    .map_err(|_| InstallError::Unavailable)?;
    options.program = CliProgram::Resolve;
    options.bundled_cli_extract_dir = Some(root.join("embedded"));
    let client = tokio::time::timeout(Duration::from_secs(45), Client::start(options))
        .await
        .map_err(|_| InstallError::Unavailable)?
        .map_err(|_| InstallError::Unavailable)?;
    let auth = tokio::time::timeout(Duration::from_secs(10), client.get_auth_status()).await;
    let unauthenticated =
        matches!(auth, Ok(Ok(status)) if !status.is_authenticated && status.login.is_none());
    let stopped = matches!(
        tokio::time::timeout(Duration::from_secs(15), client.stop()).await,
        Ok(Ok(()))
    );
    client.force_stop();
    if !unauthenticated || !stopped {
        return Err(InstallError::Unavailable);
    }
    let program = locate_runtime(root)?;
    let wrapper_sha256 = hash_file(&program)?;
    let runtime_sha256 = hash_file(
        &program
            .parent()
            .ok_or(InstallError::Unavailable)?
            .join("runtime.node"),
    )?;
    Ok(Receipt {
        version: RUNTIME_VERSION.into(),
        program,
        wrapper_sha256,
        runtime_sha256,
    })
}

/// Called before logging, settings, or the desktop runtime is initialized.
pub fn helper_main(root: &Path) -> i32 {
    let mut ready = [0u8; 8];
    if std::io::stdin().read_exact(&mut ready).is_err() || ready != READY {
        return 2;
    }
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return 2;
    };
    match runtime.block_on(install(root)) {
        Ok(receipt) => match serde_json::to_string(&receipt) {
            Ok(receipt) => {
                println!("{receipt}");
                if std::io::stdout().flush().is_err() {
                    return 2;
                }
                let mut exit = [0u8; 1];
                if std::io::stdin().read_exact(&mut exit).is_ok() && exit == *b"q" {
                    0
                } else {
                    2
                }
            }
            Err(_) => 2,
        },
        Err(_) => 2,
    }
}

#[cfg(test)]
mod tests;
