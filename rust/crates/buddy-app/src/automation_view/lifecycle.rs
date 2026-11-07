//! The coordinator's one app-level quit wait.
//!
//! A coordinator can be inside a config lock when the app quits. Exiting then leaves a fresh
//! lock, and the next start reports Busy until the lock goes stale. So quit asks the task to
//! stop and spends GPUI's existing `SHUTDOWN_TIMEOUT` waiting for it to end. The hook belongs
//! to the app, not to a view, because the last view can be gone before the app quits.
//!
//! This narrows the window and does not close it: a tick or an edit that is already sending
//! over the network can outlast the wait. Nothing here cancels that send or proves the backend
//! received it, and the worker supervisor is not part of what is waited for.
use super::service::{Command, Completion, Handle};

/// Fixed text, so no path or job detail can reach the log from here.
pub(super) const INTERRUPTED: &str =
    "The automation coordinator stopped unexpectedly. Its last change may not be saved.";

/// `Ok` means the task ended and its storage guards are gone; that includes a coordinator that
/// failed to open and reported Attention. `Err` means the task unwound or was dropped first,
/// which is never reported as a clean stop.
pub(super) async fn finished(completion: Completion) -> Result<(), &'static str> {
    completion.await.map_err(|_| INTERRUPTED)
}

/// Register once, for the coordinator this call site has just started.
pub(super) fn wait_on_quit(cx: &gpui_kit::App, handle: Handle, completion: Completion) {
    let mut completion = Some(completion);
    cx.on_app_quit(move |_| {
        handle.send(Command::Stop);
        let completion = completion.take();
        async move {
            if let Some(completion) = completion
                && let Err(message) = finished(completion).await
            {
                log::error!("{message}");
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Runtime;
    use buddy_core::automation::{engine::Phase, journal::Context};
    use buddy_core::config_lock::{ConfigLock, lock_dir};
    use futures::channel::oneshot;
    use futures::executor::block_on;
    use std::path::PathBuf;
    use std::time::Duration;

    fn controlled_panic() {
        panic!("controlled coordinator panic");
    }

    #[test]
    fn consuming_stop_is_not_completion_while_the_guard_is_held() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let lock = ConfigLock::acquire(&config, Duration::ZERO)
            .unwrap()
            .unwrap();
        let (saw_stop, stop_seen) = std::sync::mpsc::channel();
        let (release, released) = oneshot::channel::<()>();
        let runtime = Runtime::start();
        let (handle, mut completion) = Handle::spawn(&runtime, move |mut commands, _| async move {
            let _held = lock;
            while let Some(envelope) = commands.recv().await {
                if matches!(envelope.command, Command::Stop) {
                    break;
                }
            }
            saw_stop.send(()).unwrap();
            let _ = released.await;
        });

        assert!(handle.send(Command::Stop));
        stop_seen
            .recv_timeout(Duration::from_secs(30))
            .expect("the worker consumes Stop");
        // Whatever else was already queued on the runtime has its turn before this check.
        block_on(runtime.spawn(async {})).unwrap();
        assert_eq!(
            completion.try_recv(),
            Ok(None),
            "consuming Stop does not end the task"
        );
        assert!(lock_dir(&config).exists());

        release.send(()).unwrap();
        assert_eq!(block_on(finished(completion)), Ok(()));
        assert!(!lock_dir(&config).exists());
    }

    #[test]
    fn a_panicking_worker_reports_interruption_only_after_its_lock_is_released() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let lock = ConfigLock::acquire(&config, Duration::ZERO)
            .unwrap()
            .unwrap();
        let runtime = Runtime::start();
        let (handle, completion) = Handle::spawn(&runtime, move |mut commands, _| async move {
            let _held = lock;
            while let Some(envelope) = commands.recv().await {
                if matches!(envelope.command, Command::Stop) {
                    break;
                }
            }
            controlled_panic();
        });

        assert!(handle.send(Command::Stop));
        assert_eq!(
            block_on(finished(completion)),
            Err(INTERRUPTED),
            "an unwound task is never a clean stop"
        );
        assert!(!lock_dir(&config).exists());
    }

    #[test]
    fn a_coordinator_that_cannot_open_ends_its_task_and_reports_attention() {
        let runtime = Runtime::start();
        let (handle, completion) = Handle::start(
            &runtime,
            PathBuf::from("relative/config.json"),
            Context::default(),
        );

        assert_eq!(block_on(finished(completion)), Ok(()));
        let snapshot = handle.snapshot();
        assert!(snapshot.phase == Phase::Attention);
        assert!(snapshot.error.is_some());
    }
}
