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
    let (handle, future) = start(options).map_err(|_| ())?;
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
    if snapshot.busy || snapshot.cleanup_warning || snapshot.progress != Progress::Finished {
        return Err(());
    }
    if cancel {
        if snapshot.phase != Phase::Cancelled
            || snapshot.output != "Partial synthetic answer 界"
            || !cancelled
        {
            return Err(());
        }
    } else if snapshot.phase != Phase::Completed
        || !snapshot.output.contains("Unicode: 界 ✓")
        || distinct < 2
    {
        return Err(());
    }
    if saved {
        if snapshot.save != SaveState::Saved {
            return Err(());
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
            return Err(());
        }
    } else if snapshot.save != SaveState::NotRequested || snapshot.result_id.is_some() {
        return Err(());
    }
    Ok(snapshot)
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
    .map_err(|_| ())?;
    let directory = tempfile::Builder::new()
        .prefix("auth-check-")
        .tempdir_in(owned.join("turns"))
        .map_err(|_| ())?;
    let runtime = AssistantRuntime::start(program.program(), directory.path(), None)
        .await
        .map_err(|_| ())?;
    if !matches!(
        runtime.create_session("synthetic-account", "gpt-4.1").await,
        Err(Failure::AuthenticationRequired)
    ) {
        return Err(());
    }
    runtime.stop().await.map_err(|_| ())?;
    drop(runtime);
    directory.close().map_err(|_| ())?;
    let directory = tempfile::Builder::new()
        .prefix("clone-check-")
        .tempdir_in(owned.join("turns"))
        .map_err(|_| ())?;
    let fixture = crate::copilot_fixture::load(&owned)
        .map_err(|_| ())?
        .ok_or(())?;
    let runtime = AssistantRuntime::start_with_fixture(
        program.program(),
        directory.path(),
        None,
        Some(fixture),
    )
    .await
    .map_err(|_| ())?;
    let session = Arc::new(
        runtime
            .create_session("synthetic-account", "gpt-4.1")
            .await
            .map_err(|_| ())?,
    );
    let retained = session.clone();
    drop(session);
    drop(runtime);
    if !matches!(
        tokio::time::timeout(Duration::from_secs(5), retained.send("BUDDYSTREAM61527")).await,
        Ok(Err(_))
    ) {
        return Err(());
    }
    drop(retained);
    directory.close().map_err(|_| ())?;
    let remaining = std::fs::read_dir(owned.join("turns"))
        .map_err(|_| ())?
        .count();
    if remaining != 0 {
        return Err(());
    }
    Ok(
        serde_json::json!({"offline":true,"actualSdkStream":true,"unicodeDeltas":true,"unsavedNoRecord":true,"cancelPartialRetained":true,"savedOutputReadBack":true,"savedCancellationReadBack":true,"activeHandleDropStopped":true,"unauthenticatedSessionRejected":true,"retainedSessionTransportStopped":true,"temporaryTurnDirectories":remaining,"upstreamForwarder":false}),
    )
}
