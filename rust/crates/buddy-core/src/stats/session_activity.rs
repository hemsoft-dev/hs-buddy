//! Session statistics, matching Electron's `useAppSessionStats.ts`: one
//! `recordSessionStart` when the app opens, `checkpointUptime` every five
//! minutes, and `recordSessionEnd` when it quits. The mutations take no
//! arguments; Convex reads its own clock, adds the elapsed time to
//! `totalUptimeMs` and counts `appLaunches`.
//!
//! Electron sends all three over its one live Convex connection. This keeps
//! one connection for the session too, so the end of a session needs only the
//! write itself inside GPUI's short quit budget. Each event is sent at most
//! once; a failed or uncertain write is never replayed.

use std::collections::BTreeMap;
use std::time::Duration;

use convex::{ConvexClient, FunctionResult};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};

use crate::terminal_backend::BackendError;

/// Electron's `UPTIME_CHECKPOINT_MS`.
pub const UPTIME_CHECKPOINT: Duration = Duration::from_secs(5 * 60);

const WRITE_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    Start,
    Checkpoint,
    End,
}

impl SessionEvent {
    pub fn function(self) -> &'static str {
        match self {
            Self::Start => "buddyStats:recordSessionStart",
            Self::Checkpoint => "buddyStats:checkpointUptime",
            Self::End => "buddyStats:recordSessionEnd",
        }
    }
}

/// One event to send. `done` is answered once the write has finished,
/// confirmed or not, so a quit can wait for the session end.
pub struct Request {
    pub event: SessionEvent,
    pub done: Option<oneshot::Sender<()>>,
}

/// Sends each request in order over one Convex connection until the channel
/// closes. Run it on the I/O runtime.
pub async fn serve(url: String, requests: mpsc::UnboundedReceiver<Request>) {
    serve_with(requests, &mut Connection { url, client: None }).await;
}

trait Write {
    async fn write(&mut self, function: &'static str) -> Result<(), BackendError>;
}

/// Connects on the first event and keeps the connection; a failed connection
/// is tried again for the next event, never for the event that failed.
struct Connection {
    url: String,
    client: Option<ConvexClient>,
}

impl Write for Connection {
    async fn write(&mut self, function: &'static str) -> Result<(), BackendError> {
        if self.client.is_none() {
            let client = crate::settings_backend::connect(&self.url)
                .await
                .map_err(|_| BackendError::Unavailable)?;
            self.client = Some(client);
        }
        let Some(client) = self.client.as_mut() else {
            return Err(BackendError::Unavailable);
        };
        match tokio::time::timeout(WRITE_TIMEOUT, client.mutation(function, BTreeMap::new())).await
        {
            Ok(Ok(FunctionResult::Value(_))) => Ok(()),
            Ok(Ok(_)) => Err(BackendError::Rejected),
            Ok(Err(_)) | Err(_) => {
                // Drop the client: a dead worker must not fail every later
                // event, and a live one must not resend this write after a
                // reconnect. The next event connects afresh.
                self.client = None;
                Err(BackendError::UnknownWrite)
            }
        }
    }
}

async fn serve_with(mut requests: mpsc::UnboundedReceiver<Request>, writer: &mut impl Write) {
    while let Some(request) = requests.next().await {
        if writer.write(request.event.function()).await.is_err() {
            log::debug!("A session statistic was not confirmed; it will not be replayed");
        }
        if let Some(done) = request.done {
            let _ = done.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    #[derive(Default)]
    struct Fake {
        calls: Vec<&'static str>,
        failures: Vec<BackendError>,
    }

    impl Write for Fake {
        async fn write(&mut self, function: &'static str) -> Result<(), BackendError> {
            self.calls.push(function);
            match self.failures.pop() {
                Some(failure) => Err(failure),
                None => Ok(()),
            }
        }
    }

    fn request(event: SessionEvent) -> (Request, oneshot::Receiver<()>) {
        let (done, ack) = oneshot::channel();
        (
            Request {
                event,
                done: Some(done),
            },
            ack,
        )
    }

    #[test]
    fn events_call_electrons_session_mutations() {
        assert_eq!(
            SessionEvent::Start.function(),
            "buddyStats:recordSessionStart"
        );
        assert_eq!(
            SessionEvent::Checkpoint.function(),
            "buddyStats:checkpointUptime"
        );
        assert_eq!(SessionEvent::End.function(), "buddyStats:recordSessionEnd");
        assert_eq!(UPTIME_CHECKPOINT, Duration::from_millis(5 * 60 * 1000));
    }

    #[test]
    fn events_are_sent_once_in_order_and_each_is_answered() {
        let (sender, receiver) = mpsc::unbounded();
        let mut acks = Vec::new();
        for event in [
            SessionEvent::Start,
            SessionEvent::Checkpoint,
            SessionEvent::End,
        ] {
            let (request, ack) = request(event);
            sender.unbounded_send(request).unwrap();
            acks.push(ack);
        }
        drop(sender);
        let mut fake = Fake::default();
        block_on(serve_with(receiver, &mut fake));
        assert_eq!(
            fake.calls,
            [
                "buddyStats:recordSessionStart",
                "buddyStats:checkpointUptime",
                "buddyStats:recordSessionEnd",
            ]
        );
        for ack in acks {
            assert_eq!(block_on(ack), Ok(()));
        }
    }

    #[test]
    fn failed_or_uncertain_writes_are_answered_and_never_replayed() {
        for failure in [
            BackendError::Unavailable,
            BackendError::UnknownWrite,
            BackendError::Rejected,
        ] {
            let (sender, receiver) = mpsc::unbounded();
            let (start, start_ack) = request(SessionEvent::Start);
            let (end, end_ack) = request(SessionEvent::End);
            sender.unbounded_send(start).unwrap();
            sender.unbounded_send(end).unwrap();
            drop(sender);
            let mut fake = Fake {
                failures: vec![failure],
                ..Fake::default()
            };
            block_on(serve_with(receiver, &mut fake));
            // The failed start is not sent again; the end still is, once.
            assert_eq!(
                fake.calls,
                [
                    "buddyStats:recordSessionStart",
                    "buddyStats:recordSessionEnd"
                ]
            );
            assert_eq!(block_on(start_ack), Ok(()));
            assert_eq!(block_on(end_ack), Ok(()));
        }
    }
}
