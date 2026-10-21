//! Start is private pipe data. No job environment is applied to the trusted host.
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

const MAX_START: usize = 2 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", content = "config", rename_all = "lowercase")]
pub(super) enum Start {
    Exec(Box<super::exec::Spec>),
    Ai(Box<super::ai::Spec>),
    Snapshot(Box<super::snapshot::Spec>),
}
impl Start {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Exec(spec) => spec.validate(),
            Self::Ai(spec) => spec.validate(),
            Self::Snapshot(spec) => spec.validate(),
        }
    }
}

pub(super) fn send_start(writer: &mut impl Write, spec: &Start) -> io::Result<()> {
    let bytes = serde_json::to_vec(spec).map_err(io::Error::other)?;
    if bytes.len() > MAX_START {
        return Err(io::Error::other("Worker configuration exceeds its limit."));
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
        return Err(io::Error::other("Unsupported worker start packet."));
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes)?;
    let spec: Start = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::other("Unsupported worker start packet."))?;
    spec.validate().map_err(io::Error::other)?;
    Ok(spec)
}

/// App entry points dispatch here before logging, config, credentials, network or UI.
pub fn run_automation_host() -> i32 {
    match run() {
        Ok(code) => code,
        Err(_) => {
            let _ = writeln!(
                io::stderr(),
                "The automation worker could not start its configured command."
            );
            125
        }
    }
}

fn run() -> io::Result<i32> {
    let mut input = io::stdin();
    let start = receive_start(&mut input)?;
    parent_death(input);
    match start {
        Start::Exec(spec) => run_exec(*spec),
        Start::Ai(spec) => Ok(super::ai::run(*spec)),
        Start::Snapshot(spec) => Ok(super::snapshot::run(*spec)),
    }
}

fn run_exec(spec: super::exec::Spec) -> io::Result<i32> {
    let (program, arguments) = spec.shell_command();
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(&spec.directory)
        .envs(&spec.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Keep the host session, with the shell in its own process group.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
        }
    }
    let mut child = command.spawn()?;
    Ok(child.wait()?.code().unwrap_or(1))
}

fn parent_death(mut input: io::Stdin) {
    std::thread::spawn(move || {
        let mut byte = [0; 1];
        // Any additional control byte, EOF or read failure ends this worker.
        let _ = input.read(&mut byte);
        #[cfg(unix)]
        {
            let _ = crate::terminal::ownership::terminate_own_host_session();
        }
        // Windows descendants remain in the parent's kill-on-close Job object.
        std::process::exit(125);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_and_incomplete_start_packets_do_not_spawn() {
        assert!(
            receive_start(&mut std::io::Cursor::new(
                (MAX_START as u32 + 1).to_le_bytes()
            ))
            .is_err()
        );
        assert!(receive_start(&mut std::io::Cursor::new([5, 0, 0, 0, b'{'])).is_err());
        assert!(receive_start(&mut std::io::Cursor::new([0, 0, 0, 0])).is_err());
    }
}
