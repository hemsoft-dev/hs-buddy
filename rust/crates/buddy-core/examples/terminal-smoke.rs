//! Headless exercise of the production session/host transport against an explicitly supplied host.
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use buddy_core::terminal::{
    CellColor, InputStatus, Key, Modifiers, Session, SessionSnapshot, SessionStatus, ShellSpec,
    TerminalSize,
};

struct Sentinel(Child);
impl Drop for Sentinel {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn until(
    session: &Session,
    test: impl Fn(&SessionSnapshot) -> bool,
) -> io::Result<SessionSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let snapshot = session.snapshot();
        if test(&snapshot) {
            return Ok(snapshot);
        }
        if let SessionStatus::Failed(message) = snapshot.status {
            return Err(io::Error::other(message));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "Terminal condition timed out; status {:?}; visible synthetic grid: {}",
                snapshot.status,
                snapshot.screen.text()
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn command(session: &Session, text: &str) -> io::Result<()> {
    let ticket = session.paste(text, false).map_err(io::Error::other)?;
    until(session, |_| {
        session.input_status(&ticket) == Some(InputStatus::Written)
    })?;
    session
        .key(Key::Enter, Modifiers::default())
        .map_err(io::Error::other)?;
    Ok(())
}

#[cfg(windows)]
fn sentinel() -> io::Result<Sentinel> {
    use std::os::windows::process::CommandExt;
    Ok(Sentinel(
        Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 120",
            ])
            .creation_flags(0x08000000)
            .spawn()?,
    ))
}
#[cfg(unix)]
fn sentinel() -> io::Result<Sentinel> {
    Ok(Sentinel(Command::new("/bin/sleep").arg("120").spawn()?))
}

#[cfg(windows)]
struct ChildIdentity(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl ChildIdentity {
    fn open(pid: u32) -> io::Result<Self> {
        use windows_sys::Win32::System::Threading::*;
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        if unsafe { WaitForSingleObject(handle, 0) } != windows_sys::Win32::Foundation::WAIT_TIMEOUT
        {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            return Err(io::Error::other("Owned child is not running."));
        }
        Ok(Self(handle))
    }
    fn exited(&self) -> bool {
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(self.0, 2000)
                == windows_sys::Win32::Foundation::WAIT_OBJECT_0
        }
    }
}
#[cfg(windows)]
impl Drop for ChildIdentity {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(unix)]
struct ChildIdentity(i32);
#[cfg(unix)]
impl ChildIdentity {
    fn open(pid: u32) -> io::Result<Self> {
        if unsafe { libc::kill(pid as i32, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(pid as i32))
    }
    fn exited(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if unsafe { libc::kill(self.0, 0) } != 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn unicode_executable(host: &Path, directory: &Path) -> io::Result<()> {
    let folder = directory.join("shell executable 資料");
    std::fs::create_dir(&folder)?;
    #[cfg(windows)]
    let shell = {
        let source = PathBuf::from(
            std::env::var_os("SystemRoot")
                .ok_or_else(|| io::Error::other("SystemRoot unavailable"))?,
        )
        .join("System32/cmd.exe");
        let program = folder.join("owned shell 界.exe");
        std::fs::copy(&source, &program)?;
        if std::fs::read(&program)? != std::fs::read(&source)? {
            return Err(io::Error::other("Unicode fixture shell copy changed."));
        }
        ShellSpec {
            program,
            arguments: vec!["/D".into(), "/Q".into(), "/K".into()],
            directory: directory.into(),
        }
    };
    #[cfg(unix)]
    let shell = {
        use std::os::unix::fs::PermissionsExt;
        let program = folder.join("owned shell 界");
        std::fs::write(&program, b"#!/bin/sh\nexec \"$@\"\n")?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
        ShellSpec {
            program,
            arguments: vec!["/bin/sh".into(), "-i".into()],
            directory: directory.into(),
        }
    };
    let key = std::env::consts::OS;
    let settings = serde_json::json!({key:{"program":shell.program,"arguments":shell.arguments}});
    let selected = ShellSpec::configured(Some(&settings), directory).map_err(io::Error::other)?;
    if selected != shell {
        return Err(io::Error::other(
            "Platform shell selection changed the executable or arguments.",
        ));
    }
    let session = Session::spawn(
        host,
        selected,
        TerminalSize::new(100, 24).map_err(io::Error::other)?,
    )?;
    until(&session, |s| s.status == SessionStatus::Running)?;
    #[cfg(windows)]
    command(&session, "echo BUDDY_UNICODE_^EXECUTABLE_OK")?;
    #[cfg(unix)]
    command(&session, "printf 'BUDDY_UNICODE_%s\\n' EXECUTABLE_OK")?;
    until(&session, |s| {
        s.screen
            .text()
            .lines()
            .any(|line| line.trim() == "BUDDY_UNICODE_EXECUTABLE_OK")
    })?;
    session.close()
}

fn natural_exit(host: &Path, cwd: &Path) -> io::Result<()> {
    let mut shell = ShellSpec::configured(None, cwd).map_err(io::Error::other)?;
    #[cfg(windows)]
    {
        shell.arguments.insert(1, "-NoProfile".into());
    }
    #[cfg(unix)]
    {
        shell.program = "/bin/sh".into();
        shell.arguments = vec!["-i".into()];
    }
    let session = Session::spawn(
        host,
        shell,
        TerminalSize::new(100, 24).map_err(io::Error::other)?,
    )?;
    until(&session, |snapshot| {
        snapshot.status == SessionStatus::Running
    })?;
    #[cfg(windows)]
    command(
        &session,
        "$owned=Start-Process -FilePath (Get-Process -Id $PID).Path -WindowStyle Hidden -ArgumentList '-NoLogo','-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 120' -PassThru;[Console]::WriteLine('NATURAL_CHILD:'+$owned.Id)",
    )?;
    // $! is a numeric PID. Leaving it unquoted avoids Bash 3.2 treating the
    // following double quote as a history event during interactive input.
    #[cfg(unix)]
    command(&session, r"sleep 120 & printf 'NATURAL_CHILD:%s\n' $!")?;
    let snapshot = until(&session, |snapshot| {
        snapshot.screen.text().lines().any(|line| {
            line.trim()
                .strip_prefix("NATURAL_CHILD:")
                .is_some_and(|pid| pid.parse::<u32>().is_ok())
        })
    })?;
    let pid = snapshot
        .screen
        .text()
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("NATURAL_CHILD:")
                .and_then(|pid| pid.parse::<u32>().ok())
        })
        .ok_or_else(|| io::Error::other("Natural-exit child identity missing"))?;
    let child = ChildIdentity::open(pid)?;
    command(&session, "exit")?;
    until(&session, |snapshot| {
        snapshot.status == SessionStatus::Exited(0)
    })?;
    session.close()?;
    if !child.exited() {
        return Err(io::Error::other(
            "Background child survived natural shell exit.",
        ));
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let host = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("Supply the terminal host executable."))?,
    );
    let cwd = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("Supply an owned temporary workspace directory."))?,
    );
    if args.next().is_some() {
        return Err(io::Error::other("Unexpected terminal smoke arguments."));
    }
    let mut shell = ShellSpec::configured(None, &cwd).map_err(io::Error::other)?;
    eprintln!(
        "Terminal smoke shell executable: {}",
        shell.program.display()
    );
    #[cfg(windows)]
    {
        shell.arguments.insert(1, "-NoProfile".into());
    }
    #[cfg(unix)]
    {
        shell.arguments = vec!["-i".into()];
    }
    let mut sentinel = sentinel()?;
    let session = Session::spawn(
        &host,
        shell,
        TerminalSize::new(80, 24).map_err(io::Error::other)?,
    )?;
    until(&session, |s| s.status == SessionStatus::Running)?;
    #[cfg(windows)]
    command(
        &session,
        "[Console]::WriteLine(([char]27)+'[31m'+'BUDDY_'+'COLOR'+([char]27)+'[0m');[Console]::WriteLine('BUDDY_'+([char]0x754c)+([char]0xe9));[Console]::WriteLine('BUDDY_CWD:'+([BitConverter]::ToString([Text.Encoding]::UTF8.GetBytes((Get-Location).ProviderPath))).Replace('-','').ToLowerInvariant())",
    )?;
    #[cfg(unix)]
    command(
        &session,
        r"printf '\033[31mBUDDY_%s\033[0m\n' COLOR; printf 'BUDDY_\347\225\214\303\251\n'; printf 'BUDDY_CWD:'; pwd | tr -d '\n' | od -An -tx1 | tr -d ' \n'; printf '\n'",
    )?;
    let native_cwd = cwd.components().collect::<PathBuf>();
    let expected_cwd = format!(
        "BUDDY_CWD:{}",
        native_cwd
            .to_str()
            .ok_or_else(|| io::Error::other("Fixture path is not Unicode."))?
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let snapshot = until(&session, |s| {
        let text = s.screen.text();
        text.contains("BUDDY_COLOR") && text.contains("BUDDY_界é") && text.contains(&expected_cwd)
    })?;
    let color_row = (0..snapshot.screen.size.rows() as usize)
        .find(|row| {
            snapshot
                .screen
                .cells
                .iter()
                .filter(|cell| cell.row == *row)
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .contains("BUDDY_COLOR")
        })
        .ok_or_else(|| io::Error::other("Color output is absent."))?;
    let color = snapshot
        .screen
        .cells
        .iter()
        .find(|cell| cell.row == color_row && cell.text == "B")
        .ok_or_else(|| io::Error::other("Colored cell is absent."))?
        .style
        .foreground;
    if !matches!(
        color,
        CellColor::Palette(1 | 9) | CellColor::Rgb(150..=255, 0..=120, 0..=120)
    ) {
        return Err(io::Error::other(
            "Actual terminal output did not retain ANSI red.",
        ));
    }
    session
        .resize(TerminalSize::new(100, 30).map_err(io::Error::other)?)
        .map_err(io::Error::other)?;
    until(&session, |s| {
        s.screen.size == TerminalSize::new(100, 30).unwrap()
    })?;
    #[cfg(windows)]
    command(
        &session,
        "[Console]::WriteLine('BUDDY_'+[Console]::WindowWidth+'x'+[Console]::WindowHeight)",
    )?;
    #[cfg(unix)]
    command(&session, "printf 'BUDDY_SIZE:'; stty size")?;
    #[cfg(windows)]
    until(&session, |s| s.screen.text().contains("BUDDY_100x30"))?;
    #[cfg(unix)]
    until(&session, |s| s.screen.text().contains("BUDDY_SIZE:30 100"))?;
    #[cfg(windows)]
    command(
        &session,
        "[Console]::WriteLine('BUDDY_'+'WAITING');Start-Sleep -Seconds 120",
    )?;
    #[cfg(unix)]
    command(&session, r"printf 'BUDDY_%s\n' WAITING; sleep 120")?;
    until(&session, |s| s.screen.text().contains("BUDDY_WAITING"))?;
    session
        .key(
            Key::Text("c"),
            Modifiers {
                control: true,
                ..Default::default()
            },
        )
        .map_err(io::Error::other)?;
    // A write acknowledgement is not a shell-ready acknowledgement: Ctrl+C can flush
    // input already in the console. Observe its returned prompt before sending more.
    until(&session, |snapshot| {
        snapshot
            .screen
            .text()
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .is_some_and(|line| line.trim_end().ends_with(['>', '$', '#', '%']))
    })?;
    #[cfg(windows)]
    command(&session, "[Console]::WriteLine('BUDDY_'+'INTERRUPTED')")?;
    #[cfg(unix)]
    command(&session, r"printf 'BUDDY_%s\n' INTERRUPTED")?;
    until(&session, |s| s.screen.text().contains("BUDDY_INTERRUPTED"))?;
    #[cfg(windows)]
    command(
        &session,
        "$owned=Start-Process -FilePath (Get-Process -Id $PID).Path -WindowStyle Hidden -ArgumentList '-NoLogo','-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 120' -PassThru;[Console]::WriteLine('BUDDY_CHILD:'+$owned.Id)",
    )?;
    #[cfg(unix)]
    command(&session, r"sleep 120 & printf 'BUDDY_CHILD:%s\n' $!")?;
    let snapshot = until(&session, |s| {
        s.screen.text().lines().any(|line| {
            line.trim()
                .strip_prefix("BUDDY_CHILD:")
                .is_some_and(|id| id.parse::<u32>().is_ok())
        })
    })?;
    let pid = snapshot
        .screen
        .text()
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("BUDDY_CHILD:")
                .and_then(|id| id.parse::<u32>().ok())
        })
        .ok_or_else(|| io::Error::other("No owned child identity."))?;
    let owned_child = ChildIdentity::open(pid)?;
    session.close()?;
    if !owned_child.exited() {
        return Err(io::Error::other(
            "Owned terminal child survived verified session close.",
        ));
    }
    if sentinel.0.try_wait()?.is_some() {
        return Err(io::Error::other("Unrelated sentinel was terminated."));
    }
    unicode_executable(&host, &cwd)?;
    natural_exit(&host, &cwd)?;
    if sentinel.0.try_wait()?.is_some() {
        return Err(io::Error::other(
            "Natural shell exit stopped the unrelated sentinel.",
        ));
    }
    println!(
        "{{\"result\":\"passed\",\"ansiColor\":true,\"unicode\":true,\"cwd\":true,\"unicodeExecutable\":true,\"naturalExitCleanup\":true,\"actualPtyResize\":true,\"interrupt\":true,\"ownedChildExited\":true,\"unrelatedSentinelAlive\":true,\"inputWriteAcknowledgements\":true}}"
    );
    Ok(())
}
