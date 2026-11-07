//! Private controller pipes. No input, observer or production changes.
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::time::{Duration, Instant};

pub(super) struct Bridge {
    read: std::fs::File,
    write: std::fs::File,
    deadline: Instant,
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
unsafe extern "C" {
    fn poll(fds: *mut PollFd, count: u32, timeout: i32) -> i32;
}

impl Bridge {
    pub(super) fn open() -> Option<Self> {
        let raw = std::env::var("BUDDY_RECEIVE_READ").ok()?;
        assert!(
            super::fixture_guard::enabled(),
            "Receive bridge fixture refused"
        );
        let read = raw.parse::<i32>().expect("Receive bridge descriptor");
        let write = std::env::var("BUDDY_RECEIVE_WRITE")
            .expect("Receive bridge descriptor")
            .parse::<i32>()
            .expect("Receive bridge descriptor");
        assert!(read > 2 && write > 2 && read != write);
        // SAFETY: only the canonical Python owner passes these inherited pipes.
        Some(Self {
            read: unsafe { std::fs::File::from_raw_fd(read) },
            write: unsafe { std::fs::File::from_raw_fd(write) },
            // Controller owns the stricter total deadline, including startup.
            deadline: Instant::now() + Duration::from_secs(15),
        })
    }

    fn ready(&self, file: &std::fs::File, events: i16) {
        use std::os::fd::AsRawFd;
        let millis = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis();
        assert!(millis > 0, "Receive bridge deadline");
        let mut fd = PollFd {
            fd: file.as_raw_fd(),
            events,
            revents: 0,
        };
        // SAFETY: one initialized pollfd and a bounded timeout; no foreign descriptors.
        assert_eq!(
            unsafe { poll(&mut fd, 1, millis.min(i32::MAX as u128) as i32) },
            1
        );
        assert_eq!(fd.revents & events, events, "Receive bridge EOF or failure");
    }

    fn send(&mut self, frame: &[u8]) {
        self.ready(&self.write, 4);
        // All fixed frames fit PIPE_BUF and only this child writes this pipe.
        self.write.write_all(frame).expect("Receive bridge write");
    }

    fn receive(&mut self) -> Vec<u8> {
        let mut frame = Vec::new();
        loop {
            self.ready(&self.read, 1);
            let mut byte = [0];
            assert_eq!(self.read.read(&mut byte).expect("Receive bridge read"), 1);
            if byte[0] == b'\n' {
                return frame;
            }
            frame.push(byte[0]);
            assert!(frame.len() < 32, "Receive bridge oversized");
        }
    }

    pub(super) fn attach(&mut self) {
        self.send(b"ready\n");
        let frame = self.receive();
        assert!(
            frame == b"attached" || frame == b"unavailable",
            "Receive bridge phase"
        );
    }

    pub(super) fn teardown(&mut self) {
        self.send(b"dispatched\n");
        assert!(self.receive() == b"quiesced", "Receive bridge phase");
    }

    pub(super) fn complete(&mut self) {
        self.send(b"completed\n");
    }
}

pub(super) fn launch(command: &mut std::process::Command) {
    let executable = std::env::current_exe().expect("Native control executable");
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scripts/native-smoke/macos-process-receive.py");
    command
        .env("BUDDY_RECEIVER_CONTROL_EXECUTABLE", &executable)
        .arg(script)
        .arg("--native-control")
        .arg(executable)
        .arg("--evidence-root")
        .arg(std::env::var_os("RUNNER_TEMP").expect("Original CI evidence root"));
}
