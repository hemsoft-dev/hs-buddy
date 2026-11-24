//! When the session statistics are written: Electron's `useAppSessionStats`
//! records the start when the app opens, a checkpoint every five minutes
//! and the end on `beforeunload`. Here the end is a quit handler, so it gets
//! GPUI's quit budget (`SHUTDOWN_TIMEOUT`, 200 ms); a write still in flight
//! after that, or a process that is killed, leaves the session open, as a
//! crashed Electron window does, and the next checkpoint then counts that gap.
//!
//! The worker (`session_activity::serve`) names this launch's session with one
//! owner UUID. It sends the start again on each later event until Convex
//! answers it, and sends checkpoints and the end only for a session this
//! launch opened, so a quit never ends another client's session.
//!
//! The timer runs on GPUI's executor, so tests drive it with the test clock.
use std::rc::Rc;

use buddy_core::stats::session_activity::{self, Request, SessionEvent, UPTIME_CHECKPOINT};
use futures::FutureExt as _;
use futures::channel::{mpsc, oneshot};
use futures::future::LocalBoxFuture;
use gpui_kit::App;

use crate::runtime::Runtime;

/// Queues one event; the future ends when its write has finished.
pub(crate) type Sender = Rc<dyn Fn(SessionEvent) -> LocalBoxFuture<'static, ()>>;

/// Starts the app's one session: call once, after the I/O runtime is set.
pub(crate) fn install(cx: &mut App) {
    let (requests, receiver) = mpsc::unbounded::<Request>();
    let url = buddy_core::convex_data::convex_url();
    Runtime::global(cx).spawn_detached(session_activity::serve(url, receiver));
    install_with(
        cx,
        Rc::new(move |event| {
            let (done, finished) = oneshot::channel();
            // Queued now, so dropping the future does not drop the event.
            let _ = requests.unbounded_send(Request {
                event,
                done: Some(done),
            });
            async move {
                let _ = finished.await;
            }
            .boxed_local()
        }),
    );
}

pub(crate) fn install_with(cx: &mut App, send: Sender) {
    drop(send(SessionEvent::Start));
    let checkpoint = send.clone();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(UPTIME_CHECKPOINT).await;
            drop(checkpoint(SessionEvent::Checkpoint));
        }
    })
    .detach();
    cx.on_app_quit(move |_| send(SessionEvent::End)).detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Duration;

    #[gpui_kit::test]
    fn start_then_a_checkpoint_every_five_minutes_then_end_on_quit(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let log = sent.clone();
        cx.update(|cx| {
            install_with(
                cx,
                Rc::new(move |event| {
                    log.borrow_mut().push(event);
                    async {}.boxed_local()
                }),
            )
        });
        cx.run_until_parked();
        assert_eq!(*sent.borrow(), [SessionEvent::Start]);

        cx.executor()
            .advance_clock(UPTIME_CHECKPOINT - Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(
            *sent.borrow(),
            [SessionEvent::Start],
            "not before 5 minutes"
        );

        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(
            *sent.borrow(),
            [SessionEvent::Start, SessionEvent::Checkpoint]
        );

        for _ in 0..2 {
            cx.executor().advance_clock(UPTIME_CHECKPOINT);
            cx.run_until_parked();
        }
        assert_eq!(
            *sent.borrow(),
            [
                SessionEvent::Start,
                SessionEvent::Checkpoint,
                SessionEvent::Checkpoint,
                SessionEvent::Checkpoint,
            ]
        );

        cx.quit();
        assert_eq!(sent.borrow().last(), Some(&SessionEvent::End));
        assert_eq!(
            sent.borrow()
                .iter()
                .filter(|event| **event == SessionEvent::End)
                .count(),
            1
        );
    }
}
