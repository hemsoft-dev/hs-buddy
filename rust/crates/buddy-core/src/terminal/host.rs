//! Per-session supervisor. It waits for Start before creating any shell or PTY.
use std::io::{self, Read, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};

use super::TerminalSize;
use super::input::MAX_PASTE_BYTES;
use super::protocol::{Command, Event, OUTPUT_CHUNK_BYTES, read_frame, write_frame};

type Output = Arc<Mutex<io::Stdout>>;

fn expected_pty_eof(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::BrokenPipe {
        return true;
    }
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::EIO) {
        return true;
    }
    false
}

fn send(output: &Output, event: &Event) -> io::Result<()> {
    write_frame(
        &mut *output.lock().unwrap_or_else(|e| e.into_inner()),
        event,
    )
}

enum Message {
    Command(Command),
    InputEnded,
    OutputEnded,
    Failed(&'static str),
}

struct ShellChild {
    child: Box<dyn Child + Send + Sync>,
    #[cfg(unix)]
    pid: u32,
    reaped: bool,
}

impl ShellChild {
    fn cleanup(&mut self) -> io::Result<u32> {
        if self.reaped {
            return Ok(0);
        }
        #[cfg(unix)]
        if !self.reaped
            && let Err(error) = super::ownership::shell_exited(self.pid as i32)
        {
            // If another actor reaped the child, its numeric PID is no longer
            // authority to signal a session or call Child::kill.
            self.reaped = true;
            return Err(error);
        }
        #[cfg(unix)]
        let cleanup = super::ownership::signal_shell_session(self.pid as i32, libc::SIGKILL);
        if !self.reaped {
            let _ = self.child.kill();
            let status = self.child.wait()?;
            self.reaped = true;
            #[cfg(unix)]
            cleanup?;
            return Ok(status.exit_code());
        }
        #[cfg(unix)]
        cleanup?;
        Ok(0)
    }
}

impl Drop for ShellChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.cleanup();
        }
    }
}

fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows(),
        cols: size.columns(),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Called before logger, settings, network, or GPUI initialization in the host process.
/// Stdout carries only bounded binary frames. Nothing is logged from terminal contents.
pub fn run_terminal_host() -> i32 {
    match run() {
        Ok(()) => 0,
        Err(error) => {
            let message = error.to_string().chars().take(512).collect();
            let _ = write_frame(&mut io::stdout(), &Event::Error { message });
            1
        }
    }
}

fn run() -> io::Result<()> {
    let mut input = io::stdin();
    let Some(start) = read_frame::<Command>(&mut input)? else {
        return Ok(());
    };
    let size = start.size().map_err(io::Error::other)?;
    let Command::Start { shell, .. } = start else {
        return Err(io::Error::other("The terminal host requires Start first."));
    };
    shell.validate().map_err(io::Error::other)?;
    let pair = native_pty_system()
        .openpty(pty_size(size))
        .map_err(io::Error::other)?;
    let mut command = CommandBuilder::new(&shell.program);
    command.args(&shell.arguments);
    command.cwd(&shell.directory);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(io::Error::other)?;
    let pid = child
        .process_id()
        .ok_or_else(|| io::Error::other("The PTY shell has no process identity."))?;
    let mut child = ShellChild {
        child,
        #[cfg(unix)]
        pid,
        reaped: false,
    };
    #[cfg(unix)]
    if unsafe { libc::getsid(pid as i32) } != pid as i32 {
        return Err(io::Error::other(
            "The PTY shell did not enter its own session.",
        ));
    }
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
    // Taking and dropping this writer is required even when no input is ever sent.
    let mut writer = pair.master.take_writer().map_err(io::Error::other)?;
    let output = Arc::new(Mutex::new(io::stdout()));
    send(&output, &Event::Started { shell_pid: pid })?;
    let closing = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(128);
    let (write_tx, write_rx) = mpsc::sync_channel::<(u64, Vec<u8>)>(64);

    let controls = tx.clone();
    thread::Builder::new()
        .name("terminal-host-control".into())
        .spawn(move || {
            loop {
                match read_frame::<Command>(&mut input) {
                    Ok(Some(command)) => {
                        if controls.send(Message::Command(command)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = controls.send(Message::InputEnded);
                        break;
                    }
                    Err(_) => {
                        let _ =
                            controls.send(Message::Failed("The terminal control pipe is invalid."));
                        break;
                    }
                }
            }
        })?;
    let read_output = output.clone();
    let read_events = tx.clone();
    let read_thread = thread::Builder::new()
        .name("terminal-host-output".into())
        .spawn(move || {
            let mut buffer = [0; OUTPUT_CHUNK_BYTES];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(length) => {
                        if send(
                            &read_output,
                            &Event::Output {
                                bytes: buffer[..length].to_vec(),
                            },
                        )
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if expected_pty_eof(&error) => {
                        break;
                    }
                    Err(_) => {
                        let _ = read_events.send(Message::Failed("The PTY output stream failed."));
                        return;
                    }
                }
            }
            let _ = read_events.send(Message::OutputEnded);
        })?;
    let write_output = output.clone();
    let write_events = tx.clone();
    let write_closing = closing.clone();
    let write_thread=thread::Builder::new().name("terminal-host-input".into()).spawn(move || {
        while let Ok((id,bytes))=write_rx.recv() {
            if write_closing.load(Ordering::Acquire) { break; }
            if writer.write_all(&bytes).and_then(|()|writer.flush()).is_err() {
                let _=write_events.send(Message::Failed("The PTY input write failed; its result is uncertain and will not be replayed."));
                break;
            }
            if send(&write_output,&Event::InputWritten{id,bytes:bytes.len()}).is_err() { break; }
        }
    })?;
    drop(tx);
    let mut last_input = 0;
    let mut failure = None;
    let mut exit_code = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(Message::InputEnded | Message::OutputEnded | Message::Command(Command::Close)) => {
                break;
            }
            Ok(Message::Failed(message)) => {
                failure = Some(message);
                break;
            }
            Ok(Message::Command(Command::Input { id, bytes })) => {
                if id <= last_input || bytes.is_empty() || bytes.len() > MAX_PASTE_BYTES {
                    failure = Some("Invalid terminal input frame.");
                    break;
                }
                last_input = id;
                if write_tx.try_send((id, bytes)).is_err() {
                    failure = Some("Terminal input exceeded its bounded queue.");
                    break;
                }
            }
            Ok(Message::Command(command @ Command::Resize { .. })) => {
                let size = command.size().map_err(io::Error::other)?;
                pair.master
                    .resize(pty_size(size))
                    .map_err(io::Error::other)?;
                send(
                    &output,
                    &Event::Resized {
                        columns: size.columns(),
                        rows: size.rows(),
                    },
                )?;
            }
            Ok(Message::Command(Command::Start { .. })) => {
                failure = Some("The terminal host cannot be started twice.");
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        #[cfg(windows)]
        if let Some(status) = child.child.try_wait()? {
            child.reaped = true;
            exit_code = Some(status.exit_code());
            break;
        }
        #[cfg(unix)]
        if super::ownership::shell_exited(pid as i32)? {
            break;
        }
    }
    closing.store(true, Ordering::Release);
    let cleanup = child.cleanup();
    let code = exit_code.take().unwrap_or(cleanup?);
    // Tell the parent to reap this supervisor and close its Windows Job even if an
    // owned descendant retains a console handle. Its reader continues draining output.
    if let Some(message) = failure {
        send(
            &output,
            &Event::Error {
                message: message.into(),
            },
        )?;
    } else {
        send(&output, &Event::Exited { code })?;
    }
    drop(write_tx);
    drop(pair.master);
    // Parent ownership bounds supervisor shutdown. On Windows a sole Job handle also
    // contains descendants which outlive the shell; on Unix cleanup signals its SID groups.
    write_thread
        .join()
        .map_err(|_| io::Error::other("Terminal writer panicked."))?;
    read_thread
        .join()
        .map_err(|_| io::Error::other("Terminal reader panicked."))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_permission_failures_are_not_clean_terminal_eof() {
        assert!(!expected_pty_eof(&io::Error::new(
            io::ErrorKind::PermissionDenied,
            "denied"
        )));
        assert!(expected_pty_eof(&io::Error::new(
            io::ErrorKind::BrokenPipe,
            "closed"
        )));
        #[cfg(windows)]
        assert!(!expected_pty_eof(&io::Error::from_raw_os_error(5)));
        #[cfg(unix)]
        assert!(expected_pty_eof(&io::Error::from_raw_os_error(libc::EIO)));
    }
}
