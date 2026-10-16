use std::io;
use std::path::Path;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use super::protocol::{Command, Event, OUTPUT_CHUNK_BYTES, read_frame, write_frame};
use super::{
    InputBudget, InputError, Key, Modifiers, ScreenSnapshot, ShellSpec, TerminalScreen,
    TerminalSize, encode_key, encode_paste,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Starting,
    Running,
    Exited(u32),
    Failed(String),
    Closed,
}

#[derive(Debug, Clone)]
pub struct InputTicket {
    session: uuid::Uuid,
    id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStatus {
    Pending,
    Written,
    Uncertain,
}

pub struct SessionSnapshot {
    pub screen: ScreenSnapshot,
    pub status: SessionStatus,
    pub pending_input_bytes: usize,
    pub shell_pid: Option<u32>,
    pub revision: u64,
}

struct State {
    identity: uuid::Uuid,
    screen: TerminalScreen,
    status: SessionStatus,
    input: InputBudget,
    shell_pid: Option<u32>,
    revision: u64,
    desired_size: TerminalSize,
    resizing: Option<TerminalSize>,
}

impl State {
    fn input_status(&self, ticket: &InputTicket) -> Option<InputStatus> {
        if ticket.session != self.identity {
            return None;
        }
        if ticket.id <= self.input.written_through() {
            return Some(InputStatus::Written);
        }
        Some(
            if matches!(
                self.status,
                SessionStatus::Running | SessionStatus::Starting
            ) {
                InputStatus::Pending
            } else {
                InputStatus::Uncertain
            },
        )
    }
}

struct HostProcess {
    child: Child,
    slot: Option<HostSlot>,
    #[cfg(windows)]
    job: Option<super::ownership::HostJob>,
    finished: bool,
    result: Option<String>,
}

static LIVE_HOSTS: AtomicUsize = AtomicUsize::new(0);
const MAX_LIVE_HOSTS: usize = 16;

struct HostSlot(&'static AtomicUsize);

impl HostSlot {
    fn acquire(counter: &'static AtomicUsize, limit: usize) -> io::Result<Self> {
        let mut count = counter.load(Ordering::Acquire);
        loop {
            if count >= limit {
                return Err(io::Error::other(
                    "The terminal session limit has been reached. Close a session and wait for its cleanup before opening another.",
                ));
            }
            match counter.compare_exchange_weak(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(Self(counter)),
                Err(current) => count = current,
            }
        }
    }
}

impl Drop for HostSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl HostProcess {
    fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return self
                .result
                .as_ref()
                .map_or(Ok(()), |message| Err(io::Error::other(message.clone())));
        }
        let grace = Instant::now() + Duration::from_millis(500);
        while self.child.try_wait()?.is_none() && Instant::now() < grace {
            thread::sleep(Duration::from_millis(10));
        }
        #[cfg(windows)]
        let result = self.job.as_ref().map_or(Ok(()), |job| {
            job.terminate_and_wait(Duration::from_millis(1500))
        });
        #[cfg(unix)]
        let result = {
            let deadline = Instant::now() + Duration::from_millis(1500);
            while self.child.try_wait()?.is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if self.child.try_wait()?.is_none() {
                let _ = self.child.kill();
                Err(io::Error::other(
                    "Terminal supervisor cleanup timed out. Its owned-session cleanup could not be verified.",
                ))
            } else {
                Ok(())
            }
        };
        #[cfg(windows)]
        drop(self.job.take());
        let wait = self.child.wait().map(|_| ());
        if wait.is_ok() {
            self.slot.take();
        }
        self.finished = true;
        let result = result.and(wait);
        self.result = result.as_ref().err().map(ToString::to_string);
        result
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.finish();
        }
    }
}

type SharedState = Arc<Mutex<State>>;
type Host = Arc<Mutex<HostProcess>>;

fn failed(state: &SharedState, message: String) {
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(state.status, SessionStatus::Failed(_)) {
        state.status = SessionStatus::Failed(message);
    }
    state.input.close();
    state.revision = state.revision.saturating_add(1);
}

fn begin_cleanup(
    host: &Host,
    state: &SharedState,
    cleaning: &Arc<AtomicBool>,
    closing: &Arc<AtomicBool>,
) {
    closing.store(true, Ordering::Release);
    if cleaning.swap(true, Ordering::AcqRel) {
        return;
    }
    let host = host.clone();
    let state = state.clone();
    let fallback_host = host.clone();
    let fallback_state = state.clone();
    if thread::Builder::new()
        .name("terminal-reap".into())
        .spawn(move || {
            if let Err(error) = host.lock().unwrap_or_else(|e| e.into_inner()).finish() {
                failed(&state, error.to_string());
            }
        })
        .is_err()
        && let Err(error) = fallback_host
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish()
    {
        failed(&fallback_state, error.to_string());
    }
}

fn enqueue(
    state: &SharedState,
    commands: &mpsc::SyncSender<Command>,
    encode: impl FnOnce(&State) -> Result<Vec<u8>, InputError>,
) -> Result<InputTicket, InputError> {
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(state.status, SessionStatus::Running) {
        return Err(InputError::Closed);
    }
    let bytes = encode(&state)?;
    let id = state.input.reserve(bytes.len())?;
    if commands.try_send(Command::Input { id, bytes }).is_err() {
        state.input.cancel_unsent(id)?;
        return Err(InputError::Backpressure);
    }
    state.revision = state.revision.saturating_add(1);
    Ok(InputTicket {
        session: state.identity,
        id,
    })
}

pub struct Session {
    state: SharedState,
    commands: mpsc::SyncSender<Command>,
    host: Host,
    closing: Arc<AtomicBool>,
    cleaning: Arc<AtomicBool>,
}

impl Session {
    /// Blocking startup belongs on the I/O executor, never the GPUI foreground thread.
    pub fn spawn(host_executable: &Path, shell: ShellSpec, size: TerminalSize) -> io::Result<Self> {
        shell.validate().map_err(io::Error::other)?;
        // Hold the permit through startup and asynchronous retirement. A burst
        // of pane replacements cannot exceed the process budget during cleanup.
        let slot = HostSlot::acquire(&LIVE_HOSTS, MAX_LIVE_HOSTS)?;
        let mut command = ProcessCommand::new(host_executable);
        command
            .arg("--terminal-host")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW; only the trusted host exists before Start.
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        #[cfg(windows)]
        let job = match super::ownership::HostJob::assign(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("No terminal host input pipe."))?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("No terminal host output pipe."))?;
        let host = Arc::new(Mutex::new(HostProcess {
            child,
            slot: Some(slot),
            #[cfg(windows)]
            job: Some(job),
            finished: false,
            result: None,
        }));
        let state = Arc::new(Mutex::new(State {
            identity: uuid::Uuid::new_v4(),
            screen: TerminalScreen::new(size),
            status: SessionStatus::Starting,
            input: InputBudget::default(),
            shell_pid: None,
            revision: 0,
            desired_size: size,
            resizing: None,
        }));
        let closing = Arc::new(AtomicBool::new(false));
        let cleaning = Arc::new(AtomicBool::new(false));
        let (commands, queue) = mpsc::sync_channel(128);
        let session = Self {
            state: state.clone(),
            commands: commands.clone(),
            host: host.clone(),
            closing: closing.clone(),
            cleaning: cleaning.clone(),
        };
        let read_state = state.clone();
        let read_host = host.clone();
        let read_close = closing.clone();
        let read_clean = cleaning.clone();
        let read_commands = commands.clone();
        thread::Builder::new()
            .name("terminal-events".into())
            .spawn(move || {
                loop {
                    let event = match read_frame::<Event>(&mut output) {
                        Ok(Some(event)) => event,
                        Ok(None) => break,
                        Err(_) => {
                            failed(
                                &read_state,
                                "The terminal host output protocol failed.".into(),
                            );
                            break;
                        }
                    };
                    let terminal = matches!(event, Event::Exited { .. } | Event::Error { .. });
                    if let Err(message) = handle_event(&read_state, &read_commands, event) {
                        failed(&read_state, message);
                        break;
                    }
                    if terminal {
                        begin_cleanup(&read_host, &read_state, &read_clean, &read_close);
                    }
                }
                {
                    let mut state = read_state.lock().unwrap_or_else(|e| e.into_inner());
                    if matches!(
                        state.status,
                        SessionStatus::Starting | SessionStatus::Running
                    ) {
                        state.status = if read_close.load(Ordering::Acquire) {
                            SessionStatus::Closed
                        } else {
                            SessionStatus::Failed(
                                "The terminal host closed unexpectedly. Start a new session."
                                    .into(),
                            )
                        };
                    }
                    state.input.close();
                    state.revision = state.revision.saturating_add(1);
                }
                begin_cleanup(&read_host, &read_state, &read_clean, &read_close);
            })?;
        thread::Builder::new().name("terminal-commands".into()).spawn(move || {
            loop {
                if closing.load(Ordering::Acquire) { let _=write_frame(&mut input,&Command::Close); break; }
                match queue.recv_timeout(Duration::from_millis(25)) {
                    Ok(command)=>if write_frame(&mut input,&command).is_err() { failed(&state,"Terminal input delivery is uncertain and will not be replayed.".into()); break; },
                    Err(mpsc::RecvTimeoutError::Timeout)=>continue,
                    Err(mpsc::RecvTimeoutError::Disconnected)=>break,
                }
            }
            begin_cleanup(&host,&state,&cleaning,&closing);
        })?;
        // Job membership is verified and both transport workers exist before this release gate.
        commands
            .try_send(Command::Start {
                shell,
                columns: size.columns(),
                rows: size.rows(),
            })
            .map_err(|_| io::Error::other("Terminal startup queue failed."))?;
        Ok(session)
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        SessionSnapshot {
            screen: state.screen.snapshot(),
            status: state.status.clone(),
            pending_input_bytes: state.input.pending_bytes(),
            shell_pid: state.shell_pid,
            revision: state.revision,
        }
    }

    pub fn revision(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .revision
    }

    pub fn key(&self, key: Key<'_>, modifiers: Modifiers) -> Result<InputTicket, InputError> {
        enqueue(&self.state, &self.commands, |state| {
            Ok(encode_key(key, modifiers, state.screen.input_modes().1))
        })
    }

    pub fn paste(&self, text: &str, confirmed_multiline: bool) -> Result<InputTicket, InputError> {
        enqueue(&self.state, &self.commands, |state| {
            encode_paste(text, state.screen.input_modes().0, confirmed_multiline)
        })
    }

    pub fn resize(&self, size: TerminalSize) -> Result<(), InputError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(state.status, SessionStatus::Running) {
            return Err(InputError::Closed);
        }
        state.desired_size = size;
        send_resize(&mut state, &self.commands)
    }

    pub fn input_status(&self, ticket: &InputTicket) -> Option<InputStatus> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.input_status(ticket)
    }

    pub fn scroll(&self, lines: i32) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.screen.scroll(lines);
        state.revision = state.revision.saturating_add(1);
    }

    /// Explicit close waits for owned-process cleanup. Call it on a background worker.
    pub fn close(&self) -> io::Result<()> {
        self.closing.store(true, Ordering::Release);
        let result = self.host.lock().unwrap_or_else(|e| e.into_inner()).finish();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.input.close();
        state.status = match &result {
            Ok(()) => SessionStatus::Closed,
            Err(error) => SessionStatus::Failed(error.to_string()),
        };
        state.revision = state.revision.saturating_add(1);
        result
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        begin_cleanup(&self.host, &self.state, &self.cleaning, &self.closing);
    }
}

fn send_resize(state: &mut State, commands: &mpsc::SyncSender<Command>) -> Result<(), InputError> {
    if state.resizing.is_some() || state.screen.size() == state.desired_size {
        return Ok(());
    }
    let size = state.desired_size;
    commands
        .try_send(Command::Resize {
            columns: size.columns(),
            rows: size.rows(),
        })
        .map_err(|_| InputError::Backpressure)?;
    state.resizing = Some(size);
    Ok(())
}

fn handle_event(
    state: &SharedState,
    commands: &mpsc::SyncSender<Command>,
    event: Event,
) -> Result<(), String> {
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    match event {
        Event::Started { shell_pid }
            if shell_pid > 1 && matches!(state.status, SessionStatus::Starting) =>
        {
            state.shell_pid = Some(shell_pid);
            state.status = SessionStatus::Running;
        }
        Event::Output { bytes }
            if state.shell_pid.is_some() && bytes.len() <= OUTPUT_CHUNK_BYTES =>
        {
            state.screen.feed(&bytes);
            if state.screen.response_overflow() {
                return Err(
                    "Terminal response traffic exceeded its limit. Restart this session.".into(),
                );
            }
            for reply in state.screen.take_replies() {
                if !matches!(state.status, SessionStatus::Running) {
                    break;
                }
                let id = state
                    .input
                    .reserve(reply.len())
                    .map_err(|e| e.to_string())?;
                if commands
                    .try_send(Command::Input { id, bytes: reply })
                    .is_err()
                {
                    state.input.cancel_unsent(id).map_err(|e| e.to_string())?;
                    return Err("The terminal response queue is full.".into());
                }
            }
        }
        Event::InputWritten { id, bytes } => state
            .input
            .acknowledge(id, bytes)
            .map_err(|e| e.to_string())?,
        Event::Resized { columns, rows } => {
            let size = TerminalSize::new(columns, rows).map_err(String::from)?;
            if state.resizing != Some(size) {
                return Err(
                    "The terminal resize acknowledgement does not match its request.".into(),
                );
            }
            state.screen.resize(size);
            state.resizing = None;
            send_resize(&mut state, commands).map_err(|e| e.to_string())?;
        }
        Event::Exited { code } => {
            state.input.close();
            if !matches!(state.status, SessionStatus::Failed(_)) {
                state.status = SessionStatus::Exited(code);
            }
        }
        Event::Error { message } => {
            state.input.close();
            state.status = SessionStatus::Failed(message.chars().take(512).collect());
        }
        _ => return Err("An unexpected terminal host event was rejected.".into()),
    }
    state.revision = state.revision.saturating_add(1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_budget_reserves_startup_and_retiring_sessions_until_released() {
        static TEST_HOSTS: AtomicUsize = AtomicUsize::new(0);
        let first = HostSlot::acquire(&TEST_HOSTS, 2).unwrap();
        let second = HostSlot::acquire(&TEST_HOSTS, 2).unwrap();
        assert!(HostSlot::acquire(&TEST_HOSTS, 2).is_err());
        drop(first);
        let replacement = HostSlot::acquire(&TEST_HOSTS, 2).unwrap();
        assert!(HostSlot::acquire(&TEST_HOSTS, 2).is_err());
        drop(second);
        drop(replacement);
        assert_eq!(TEST_HOSTS.load(Ordering::Acquire), 0);
    }

    fn running() -> SharedState {
        let size = TerminalSize::new(80, 24).unwrap();
        Arc::new(Mutex::new(State {
            identity: uuid::Uuid::new_v4(),
            screen: TerminalScreen::new(size),
            status: SessionStatus::Running,
            input: InputBudget::default(),
            shell_pid: Some(42),
            revision: 0,
            desired_size: size,
            resizing: None,
        }))
    }

    #[test]
    fn write_receipt_requires_the_exact_ack_and_cannot_transfer_between_sessions() {
        let state = running();
        let (commands, queue) = mpsc::sync_channel(128);
        let ticket = enqueue(&state, &commands, |_| Ok(b"one".to_vec())).unwrap();
        assert_eq!(
            state.lock().unwrap().input_status(&ticket),
            Some(InputStatus::Pending)
        );
        let Command::Input { id, bytes } = queue.recv().unwrap() else {
            panic!("wrong command");
        };
        assert_eq!(bytes, b"one");
        handle_event(&state, &commands, Event::InputWritten { id, bytes: 3 }).unwrap();
        assert_eq!(
            state.lock().unwrap().input_status(&ticket),
            Some(InputStatus::Written)
        );
        assert_eq!(running().lock().unwrap().input_status(&ticket), None);
        let pending = enqueue(&state, &commands, |_| Ok(b"unconfirmed".to_vec())).unwrap();
        failed(&state, "lost host".into());
        assert_eq!(
            state.lock().unwrap().input_status(&pending),
            Some(InputStatus::Uncertain)
        );
        assert_eq!(
            state.lock().unwrap().input_status(&ticket),
            Some(InputStatus::Written)
        );
    }

    #[test]
    fn paste_mode_is_checked_before_reservation_and_bad_ack_is_never_written() {
        let state = running();
        let (commands, queue) = mpsc::sync_channel(128);
        assert!(matches!(
            enqueue(&state, &commands, |state| encode_paste(
                "one\ntwo",
                state.screen.input_modes().0,
                false
            )),
            Err(InputError::ConfirmMultiline)
        ));
        assert!(queue.try_recv().is_err());
        assert_eq!(state.lock().unwrap().input.pending_bytes(), 0);
        state.lock().unwrap().screen.feed(b"\x1b[?2004h");
        let ticket = enqueue(&state, &commands, |state| {
            encode_paste("one\ntwo", state.screen.input_modes().0, false)
        })
        .unwrap();
        let Command::Input { id, bytes } = queue.recv().unwrap() else {
            panic!("wrong command");
        };
        assert_eq!(bytes, b"\x1b[200~one\rtwo\x1b[201~");
        assert!(handle_event(&state, &commands, Event::InputWritten { id, bytes: 1 }).is_err());
        assert_ne!(
            state.lock().unwrap().input_status(&ticket),
            Some(InputStatus::Written)
        );
        assert!(matches!(
            enqueue(&state, &commands, |_| Ok(b"next".to_vec())),
            Err(InputError::Closed)
        ));
    }

    #[test]
    fn resized_grid_changes_only_on_matching_ack_and_latest_request_is_coalesced() {
        let state = running();
        let (commands, queue) = mpsc::sync_channel(128);
        {
            let mut state = state.lock().unwrap();
            state.desired_size = TerminalSize::new(100, 30).unwrap();
            send_resize(&mut state, &commands).unwrap();
            state.desired_size = TerminalSize::new(120, 40).unwrap();
            send_resize(&mut state, &commands).unwrap();
            assert_eq!(state.screen.size(), TerminalSize::new(80, 24).unwrap());
        }
        assert!(matches!(
            queue.recv().unwrap(),
            Command::Resize {
                columns: 100,
                rows: 30
            }
        ));
        assert!(queue.try_recv().is_err());
        handle_event(
            &state,
            &commands,
            Event::Resized {
                columns: 100,
                rows: 30,
            },
        )
        .unwrap();
        assert_eq!(
            state.lock().unwrap().screen.size(),
            TerminalSize::new(100, 30).unwrap()
        );
        assert!(matches!(
            queue.recv().unwrap(),
            Command::Resize {
                columns: 120,
                rows: 40
            }
        ));
        assert!(
            handle_event(
                &state,
                &commands,
                Event::Resized {
                    columns: 99,
                    rows: 40
                }
            )
            .is_err()
        );
        assert_eq!(
            state.lock().unwrap().screen.size(),
            TerminalSize::new(100, 30).unwrap()
        );
        handle_event(
            &state,
            &commands,
            Event::Resized {
                columns: 120,
                rows: 40,
            },
        )
        .unwrap();
        assert_eq!(
            state.lock().unwrap().screen.size(),
            TerminalSize::new(120, 40).unwrap()
        );
    }
}
