//! Tokio runtime on a dedicated thread for I/O (reqwest, gh, Convex). Results
//! cross back to GPUI over executor-agnostic `futures` channels.

use std::future::Future;

use futures::channel::oneshot;
use gpui_kit::{App, Global};

pub struct Runtime {
    executor: Executor,
}

enum Executor {
    Io(tokio::runtime::Handle),
    #[cfg(test)]
    Test(gpui_kit::BackgroundExecutor),
}

impl Global for Runtime {}

impl Runtime {
    pub fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("buddy-io")
            .enable_all()
            .build()
            .expect("tokio runtime");
        let handle = runtime.handle().clone();
        std::thread::Builder::new()
            .name("buddy-tokio".into())
            .spawn(move || runtime.block_on(std::future::pending::<()>()))
            .expect("tokio driver thread");
        Self {
            executor: Executor::Io(handle),
        }
    }

    /// Keep fake I/O on GPUI's deterministic scheduler in headless view tests.
    #[cfg(test)]
    pub fn for_test(executor: gpui_kit::BackgroundExecutor) -> Self {
        Self {
            executor: Executor::Test(executor),
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Run a future on the I/O runtime; await the receiver from a GPUI task.
    pub fn spawn<F>(&self, future: F) -> oneshot::Receiver<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.spawn_detached(async move {
            let _ = tx.send(future.await);
        });
        rx
    }

    /// Fire-and-forget on the I/O runtime (long-lived subscriptions).
    pub fn spawn_detached<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match &self.executor {
            Executor::Io(handle) => {
                handle.spawn(future);
            }
            #[cfg(test)]
            Executor::Test(executor) => executor.spawn(future).detach(),
        }
    }
}
