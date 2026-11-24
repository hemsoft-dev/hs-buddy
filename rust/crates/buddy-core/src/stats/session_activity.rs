//! Session statistics, matching Electron's `useAppSessionStats.ts`: a session
//! start when the app opens, an uptime checkpoint every five minutes, and a
//! session end when it quits. Convex reads its own clock, adds the elapsed
//! time to `totalUptimeMs` and counts `appLaunches`.
//!
//! The native mutations carry one UUID per launch (`owner`). A start is a
//! no-op when it was already applied for this owner, so an unconfirmed start
//! is sent again with the same owner on each later event until Convex answers
//! it. Checkpoints and the end are sent only once a start returned
//! `started: true`: when another client (Electron, or another native launch)
//! already holds the open session, this launch leaves it alone and sends no
//! checkpoint or end, so its quit never closes that session.
//!
//! Electron sends all three over its one live Convex connection. This keeps
//! one connection for the session too, so the end of a session needs only the
//! write itself inside GPUI's short quit budget. A failed or uncertain
//! checkpoint or end is never replayed.

use std::collections::BTreeMap;
use std::time::Duration;

use convex::{ConvexClient, FunctionResult};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use serde_json::Value;

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
            Self::Start => "buddyStats:recordSessionStartNative",
            Self::Checkpoint => "buddyStats:checkpointUptimeNative",
            Self::End => "buddyStats:recordSessionEndNative",
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
/// closes, under a fresh launch owner. Run it on the I/O runtime.
pub async fn serve(url: String, requests: mpsc::UnboundedReceiver<Request>) {
    let owner = uuid::Uuid::new_v4().to_string();
    serve_with(&owner, requests, &mut Connection { url, client: None }).await;
}

trait Write {
    async fn write(&mut self, function: &'static str, owner: &str) -> Result<Value, BackendError>;
}

/// Connects on the first event and keeps the connection; a failed connection
/// is tried again for the next event, never for the event that failed.
struct Connection {
    url: String,
    client: Option<ConvexClient>,
}

impl Write for Connection {
    async fn write(&mut self, function: &'static str, owner: &str) -> Result<Value, BackendError> {
        if self.client.is_none() {
            let client = crate::settings_backend::connect(&self.url)
                .await
                .map_err(|_| BackendError::Unavailable)?;
            self.client = Some(client);
        }
        let Some(client) = self.client.as_mut() else {
            return Err(BackendError::Unavailable);
        };
        let arguments = BTreeMap::from([("owner".to_owned(), convex::Value::from(owner))]);
        match tokio::time::timeout(WRITE_TIMEOUT, client.mutation(function, arguments)).await {
            Ok(Ok(FunctionResult::Value(value))) => Ok(Value::from(value)),
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

/// Whether this launch holds the open session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ownership {
    /// No start has been confirmed yet; the next event sends it again.
    Unconfirmed,
    Owned,
    /// Another client's session was open at the start.
    Other,
}

async fn start(writer: &mut impl Write, owner: &str) -> Ownership {
    match writer.write(SessionEvent::Start.function(), owner).await {
        Ok(reply) => match reply.get("started").and_then(Value::as_bool) {
            Some(true) => Ownership::Owned,
            Some(false) => Ownership::Other,
            None => Ownership::Unconfirmed,
        },
        Err(_) => Ownership::Unconfirmed,
    }
}

async fn serve_with(
    owner: &str,
    mut requests: mpsc::UnboundedReceiver<Request>,
    writer: &mut impl Write,
) {
    let mut ownership = Ownership::Unconfirmed;
    while let Some(request) = requests.next().await {
        if ownership == Ownership::Unconfirmed {
            ownership = start(writer, owner).await;
        }
        if request.event != SessionEvent::Start
            && ownership == Ownership::Owned
            && writer.write(request.event.function(), owner).await.is_err()
        {
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
    use serde_json::json;
    use std::collections::VecDeque;

    const OWNER: &str = "9f8e7d6c-5b4a-4c3d-9e2f-1a0b9c8d7e6f";
    const START: &str = "buddyStats:recordSessionStartNative";
    const CHECKPOINT: &str = "buddyStats:checkpointUptimeNative";
    const END: &str = "buddyStats:recordSessionEndNative";

    /// Answers starts from `starts` (then `started: true`) and fails the
    /// other writes from `failures` (then succeeds).
    #[derive(Default)]
    struct Fake {
        calls: Vec<(&'static str, String)>,
        starts: VecDeque<Result<Value, BackendError>>,
        failures: Vec<BackendError>,
    }

    impl Write for Fake {
        async fn write(
            &mut self,
            function: &'static str,
            owner: &str,
        ) -> Result<Value, BackendError> {
            self.calls.push((function, owner.to_owned()));
            if function == START {
                return self
                    .starts
                    .pop_front()
                    .unwrap_or_else(|| Ok(json!({"started": true})));
            }
            match self.failures.pop() {
                Some(failure) => Err(failure),
                None => Ok(Value::Null),
            }
        }
    }

    impl Fake {
        fn functions(&self) -> Vec<&'static str> {
            self.calls.iter().map(|(function, _)| *function).collect()
        }
    }

    /// Serves `events` and returns once each has been answered.
    fn serve_events(fake: &mut Fake, events: &[SessionEvent]) {
        let (sender, receiver) = mpsc::unbounded();
        let mut acks = Vec::new();
        for &event in events {
            let (done, ack) = oneshot::channel();
            sender
                .unbounded_send(Request {
                    event,
                    done: Some(done),
                })
                .unwrap();
            acks.push(ack);
        }
        drop(sender);
        block_on(serve_with(OWNER, receiver, fake));
        for ack in acks {
            assert_eq!(block_on(ack), Ok(()));
        }
    }

    use SessionEvent::{Checkpoint, End, Start};

    const FAILURES: [fn() -> BackendError; 3] = [
        || BackendError::Unavailable,
        || BackendError::UnknownWrite,
        || BackendError::Rejected,
    ];

    #[test]
    fn events_call_the_native_session_mutations() {
        assert_eq!(Start.function(), START);
        assert_eq!(Checkpoint.function(), CHECKPOINT);
        assert_eq!(End.function(), END);
        assert_eq!(UPTIME_CHECKPOINT, Duration::from_millis(5 * 60 * 1000));
    }

    #[test]
    fn events_are_sent_once_in_order_under_one_owner() {
        let mut fake = Fake::default();
        serve_events(&mut fake, &[Start, Checkpoint, End]);
        assert_eq!(fake.functions(), [START, CHECKPOINT, END]);
        assert!(fake.calls.iter().all(|(_, owner)| owner == OWNER));
    }

    #[test]
    fn an_unconfirmed_start_is_retried_with_the_same_owner_until_acknowledged() {
        for failure in FAILURES {
            let mut fake = Fake {
                starts: VecDeque::from([Err(failure()), Err(failure())]),
                ..Fake::default()
            };
            serve_events(&mut fake, &[Start, Checkpoint, Checkpoint, Checkpoint, End]);
            // Two ticks only retry the start; the third start is confirmed
            // and that tick's checkpoint follows it.
            assert_eq!(
                fake.functions(),
                [START, START, START, CHECKPOINT, CHECKPOINT, END]
            );
            assert!(fake.calls.iter().all(|(_, owner)| owner == OWNER));
        }
    }

    #[test]
    fn a_start_that_is_never_confirmed_sends_no_checkpoint_or_end() {
        let mut fake = Fake {
            starts: VecDeque::from([
                Err(BackendError::Unavailable),
                Ok(Value::Null),
                Err(BackendError::UnknownWrite),
            ]),
            ..Fake::default()
        };
        serve_events(&mut fake, &[Start, Checkpoint, End]);
        assert_eq!(fake.functions(), [START, START, START]);
    }

    #[test]
    fn another_clients_session_gets_no_checkpoint_or_end() {
        let mut fake = Fake {
            starts: VecDeque::from([Ok(json!({"started": false}))]),
            ..Fake::default()
        };
        serve_events(&mut fake, &[Start, Checkpoint, Checkpoint, End]);
        // The start is not retried either: Convex answered it.
        assert_eq!(fake.functions(), [START]);
    }

    #[test]
    fn failed_or_uncertain_checkpoints_and_ends_are_never_replayed() {
        for failure in FAILURES {
            let mut fake = Fake {
                failures: vec![failure(), failure()],
                ..Fake::default()
            };
            serve_events(&mut fake, &[Start, Checkpoint, Checkpoint, End]);
            assert_eq!(fake.functions(), [START, CHECKPOINT, CHECKPOINT, END]);
        }
    }
}
