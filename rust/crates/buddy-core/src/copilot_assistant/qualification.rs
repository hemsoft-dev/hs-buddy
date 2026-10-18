//! Headless installed-binary exercise. Runs only with the offline CI fixture guard.
use super::*;

pub fn main(owned: PathBuf) -> i32 {
    if !matches!(crate::copilot_fixture::load(&owned), Ok(Some(_))) {
        eprintln!("Offline Copilot qualification guard rejected this invocation.");
        return 2;
    }
    let result = tokio::runtime::Runtime::new()
        .map_err(|_| ())
        .and_then(|runtime| runtime.block_on(exercise(owned)).map_err(|_| ()));
    match result {
        Ok(receipt) => {
            println!("{receipt}");
            0
        }
        Err(()) => {
            eprintln!("Offline Copilot qualification failed.");
            1
        }
    }
}
async fn turn(owned: &std::path::Path, saved: bool, cancel: bool) -> Result<Snapshot, ()> {
    let backend = if saved {
        Some(CopilotBackend::new(&crate::convex_data::convex_url()).map_err(|_| ())?)
    } else {
        None
    };
    let options = RunOptions {
        context: Context {
            account: "synthetic-account".into(),
            model: "gpt-4.1".into(),
            review: None,
        },
        prompt: if cancel {
            "BUDDYCANCEL73429".into()
        } else {
            "BUDDYSTREAM61527".into()
        },
        saved,
        review: None,
        backend: backend.clone(),
        executable: std::env::current_exe().map_err(|_| ())?,
        owned_directory: owned.to_owned(),
    };
    let (handle, future) = start(options).map_err(|failure| {
        eprintln!("Offline SDK turn could not start: {failure:?}");
    })?;
    let mut updates = handle.subscribe();
    let mut task = tokio::spawn(future);
    let mut distinct = 0;
    let mut previous = String::new();
    let mut cancelled = false;
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        while handle.snapshot().busy {
            updates.changed().await.map_err(|_| ())?;
            let snapshot = handle.snapshot();
            if !snapshot.output.is_empty() && snapshot.output != previous {
                distinct += 1;
                previous = snapshot.output;
            }
            if cancel && !cancelled && !previous.is_empty() {
                if !handle.cancel() {
                    return Err(());
                }
                cancelled = true;
            }
        }
        (&mut task).await.map_err(|_| ())?;
        Ok(())
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        eprintln!(
            "{}",
            failure_details(&handle.snapshot(), distinct, cancelled, "turn-task")
        );
        handle.cancel();
        if !task.is_finished()
            && tokio::time::timeout(Duration::from_secs(30), &mut task)
                .await
                .is_err()
        {
            task.abort();
        }
        return Err(());
    }
    let snapshot = handle.snapshot();
    let failed = |check| {
        eprintln!("{}", failure_details(&snapshot, distinct, cancelled, check));
        Err(())
    };
    if snapshot.busy || snapshot.cleanup_warning || snapshot.progress != Progress::Finished {
        return failed("turn-cleanup");
    }
    if cancel {
        if snapshot.phase != Phase::Cancelled
            || snapshot.output != "Partial synthetic answer 界"
            || !cancelled
        {
            return failed("cancel-outcome");
        }
    } else if snapshot.phase != Phase::Completed
        || !snapshot.output.contains("Unicode: 界 ✓")
        || distinct < 2
    {
        return failed("stream-outcome");
    }
    if saved {
        if snapshot.save != SaveState::Saved {
            return failed("save-acknowledgment");
        }
        let record = backend
            .ok_or(())?
            .detail(snapshot.result_id.as_deref().ok_or(())?)
            .await
            .map_err(|_| ())?
            .ok_or(())?;
        if record.result.as_deref() != Some(snapshot.output.as_str())
            || record.status
                != if cancel {
                    crate::copilot_results::Status::Failed
                } else {
                    crate::copilot_results::Status::Completed
                }
        {
            return failed("save-readback");
        }
    } else if snapshot.save != SaveState::NotRequested || snapshot.result_id.is_some() {
        return failed("unsaved-record-absence");
    }
    Ok(snapshot)
}

pub(super) fn preparation_failure_details(
    error: &crate::copilot_runtime::InstallError,
) -> Option<String> {
    let crate::copilot_runtime::InstallError::CleanupFailed { cancelled, failure } = error else {
        return None;
    };
    Some(format!(
        "Offline SDK preparation cleanup: stage={:?}, kind={:?}, os_code={:?}, cancelled={cancelled}",
        failure.stage, failure.kind, failure.os_code
    ))
}

/// Qualification errors expose state codes, never the private Snapshot fields.
fn failure_details(
    snapshot: &Snapshot,
    deltas: usize,
    cancelled: bool,
    check: &'static str,
) -> String {
    format!(
        "Offline SDK turn check {check}: phase={:?}, progress={:?}, failure={:?}, save={:?}, busy={}, cleanup_warning={}, deltas={deltas}, cancellation_requested={cancelled}",
        snapshot.phase,
        snapshot.progress,
        snapshot.failure,
        snapshot.save,
        snapshot.busy,
        snapshot.cleanup_warning
    )
}

async fn exercise(owned: PathBuf) -> Result<serde_json::Value, ()> {
    eprintln!("Offline SDK qualification: unsaved stream");
    turn(&owned, false, false).await?;
    eprintln!("Offline SDK qualification: unsaved cancellation");
    turn(&owned, false, true).await?;
    eprintln!("Offline SDK qualification: saved stream and read-back");
    turn(&owned, true, false).await?;
    eprintln!("Offline SDK qualification: saved cancellation and read-back");
    turn(&owned, true, true).await?;
    eprintln!("Offline SDK qualification: drop a handle during an active response");
    let (handle, future) = start(RunOptions {
        context: Context {
            account: "synthetic-account".into(),
            model: "gpt-4.1".into(),
            review: None,
        },
        prompt: "BUDDYCANCEL73429".into(),
        saved: false,
        review: None,
        backend: None,
        executable: std::env::current_exe().map_err(|_| ())?,
        owned_directory: owned.clone(),
    })
    .map_err(|_| ())?;
    let mut updates = handle.subscribe();
    let mut active = tokio::spawn(future);
    let started = tokio::time::timeout(Duration::from_secs(180), async {
        while handle.snapshot().output.is_empty() {
            updates.changed().await.map_err(|_| ())?;
            if !handle.snapshot().busy {
                return Err(());
            }
        }
        Ok(())
    })
    .await;
    drop(handle);
    if !matches!(
        tokio::time::timeout(Duration::from_secs(30), &mut active).await,
        Ok(Ok(()))
    ) {
        active.abort();
        return Err(());
    }
    if !matches!(started, Ok(Ok(()))) {
        return Err(());
    }
    eprintln!("Offline SDK qualification: unauthenticated session rejection");
    let program = crate::copilot_runtime::prepare(
        std::env::current_exe().map_err(|_| ())?,
        owned.join("runtime"),
    )
    .await
    .map_err(|error| {
        eprintln!("Offline SDK auth preparation failed: {error:?}");
    })?;
    eprintln!("Offline SDK qualification: create unauthenticated state directory");
    let directory = tempfile::Builder::new()
        .prefix("auth-check-")
        .tempdir_in(owned.join("turns"))
        .map_err(|error| report_io_failure("auth-directory-create", &error))?;
    eprintln!("Offline SDK qualification: start unauthenticated transport");
    let runtime = AssistantRuntime::start(program.program(), directory.path(), None)
        .await
        .map_err(|failure| {
            eprintln!("Offline SDK unauthenticated transport failed: {failure:?}");
        })?;
    eprintln!("Offline SDK qualification: reject unauthenticated session");
    match runtime.create_session("synthetic-account", "gpt-4.1").await {
        Err(Failure::AuthenticationRequired) => {}
        Err(failure) => {
            eprintln!("Offline SDK authentication rejection returned: {failure:?}");
            return Err(());
        }
        Ok(_) => {
            eprintln!("Offline SDK authentication rejection unexpectedly created a session.");
            return Err(());
        }
    }
    eprintln!("Offline SDK qualification: stop unauthenticated transport");
    runtime.stop().await.map_err(|failure| {
        eprintln!("Offline SDK unauthenticated stop failed: {failure:?}");
    })?;
    drop(runtime);
    eprintln!("Offline SDK qualification: close unauthenticated state directory");
    directory
        .close()
        .map_err(|error| report_io_failure("auth-directory-close", &error))?;
    eprintln!("Offline SDK qualification: create retained-session state directory");
    let directory = tempfile::Builder::new()
        .prefix("clone-check-")
        .tempdir_in(owned.join("turns"))
        .map_err(|error| report_io_failure("clone-directory-create", &error))?;
    eprintln!("Offline SDK qualification: load retained-session fixture");
    let fixture = crate::copilot_fixture::load(&owned)
        .map_err(|_| ())?
        .ok_or(())?;
    eprintln!("Offline SDK qualification: start retained-session transport");
    let runtime = AssistantRuntime::start_with_fixture(
        program.program(),
        directory.path(),
        None,
        Some(fixture),
    )
    .await
    .map_err(|failure| {
        eprintln!("Offline SDK retained-session transport failed: {failure:?}");
    })?;
    eprintln!("Offline SDK qualification: create retained session");
    let session = Arc::new(
        runtime
            .create_session("synthetic-account", "gpt-4.1")
            .await
            .map_err(|failure| {
                eprintln!("Offline SDK retained-session creation failed: {failure:?}");
            })?,
    );
    let retained = session.clone();
    drop(session);
    eprintln!("Offline SDK qualification: drop runtime while retaining session");
    drop(runtime);
    if !matches!(
        tokio::time::timeout(Duration::from_secs(5), retained.send("BUDDYSTREAM61527")).await,
        Ok(Err(_))
    ) {
        eprintln!("Offline SDK retained session did not reject sending after runtime drop.");
        return Err(());
    }
    drop(retained);
    eprintln!("Offline SDK qualification: close retained-session state directory");
    directory
        .close()
        .map_err(|error| report_io_failure("clone-directory-close", &error))?;
    eprintln!("Offline SDK qualification: verify no temporary turn directories remain");
    let remaining = std::fs::read_dir(owned.join("turns"))
        .map_err(|error| report_io_failure("turn-directories-read", &error))?
        .count();
    if remaining != 0 {
        eprintln!("Offline SDK temporary turn directories remaining: {remaining}");
        return Err(());
    }
    Ok(
        serde_json::json!({"offline":true,"actualSdkStream":true,"unicodeDeltas":true,"unsavedNoRecord":true,"cancelPartialRetained":true,"savedOutputReadBack":true,"savedCancellationReadBack":true,"activeHandleDropStopped":true,"unauthenticatedSessionRejected":true,"retainedSessionTransportStopped":true,"temporaryTurnDirectories":remaining,"upstreamForwarder":false}),
    )
}

fn report_io_failure(stage: &'static str, error: &std::io::Error) {
    eprintln!(
        "Offline SDK check {stage}: kind={:?}, os_code={:?}",
        error.kind(),
        error.raw_os_error()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_failure_reports_safe_stages_and_codes_only() {
        use crate::copilot_runtime::{CleanupFailure, CleanupStage, InstallError};
        let error = InstallError::CleanupFailed {
            cancelled: false,
            failure: CleanupFailure {
                stage: CleanupStage::Reap,
                kind: std::io::ErrorKind::Other,
                os_code: Some(10),
            },
        };
        let details = preparation_failure_details(&error).unwrap();
        assert!(details.contains("Reap") && details.contains("Some(10)"));
        assert!(preparation_failure_details(&InstallError::Unavailable).is_none());
    }

    #[test]
    fn failure_diagnostics_include_state_codes_without_output_or_record_identity() {
        let private = "synthetic-private-answer-and-record";
        let snapshot = Snapshot {
            phase: Phase::Failed,
            progress: Progress::Finished,
            output: private.into(),
            failure: Some(Failure::ServiceUnavailable),
            result_id: Some(private.into()),
            save: SaveState::NotRequested,
            busy: false,
            cleanup_warning: false,
        };
        let text = failure_details(&snapshot, 0, false, "cancel-outcome");
        assert!(text.contains("cancel-outcome") && text.contains("ServiceUnavailable"));
        assert!(!text.contains(private));
    }
}
