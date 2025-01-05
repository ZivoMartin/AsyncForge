use std::sync::Arc;

use tokio::{
    sync::mpsc::error::SendError,
    task::JoinHandle,
    time::{timeout, Duration},
};

use crate::{
    dispatch_center::{MessageSender, SmartChannelId},
    error::{DispatchError, UnexpectedErrorKind},
    Sender,
};

type Handler<M> = JoinHandle<Result<(), SendError<M>>>;

/// `WritingHandler` is responsible for managing and awaiting multiple asynchronous
/// tasks that send messages via Tokio channels.
/// It allows for broadcasting messages to multiple senders and waiting for all tasks to complete.
/// You can either broadcast a message behind an `Arc` (for efficiency) or clone the message.
#[derive(Default)]
pub(crate) struct WritingHandler<M: Send + 'static> {
    handlers: Vec<Handler<M>>,
}

fn get_handler<M: Send + 'static>(sender: Sender<M, SmartChannelId>, msg: M) -> Handler<M> {
    tokio::spawn(async move { sender.send(msg).await })
}

impl<M: Send + 'static + Sync> WritingHandler<Arc<M>> {
    /// Creates a `WritingHandler` for broadcasting messages across multiple senders using `Arc<M>`.
    /// This avoids cloning the message for each sender but requires `M` to implement `Sync`.
    /// This approach is efficient for large messages.
    pub(crate) fn new_arc_broadcast(msg: M, senders: &[MessageSender<M>]) -> Self {
        let msg = Arc::new(msg);
        WritingHandler {
            handlers: senders
                .iter()
                .map(|sender| {
                    let msg = Arc::clone(&msg);
                    let sender = sender.clone();
                    get_handler(sender, msg)
                })
                .collect(),
        }
    }
}
impl<M: Send + 'static + Clone> WritingHandler<M> {
    /// Creates a `WritingHandler` by cloning the message for each sender.
    /// This is useful when sending simple notification messages.
    pub(crate) fn new_cloning_broadcast(msg: M, senders: &[Sender<M, SmartChannelId>]) -> Self {
        WritingHandler {
            handlers: senders
                .iter()
                .map(|sender| {
                    let msg = msg.clone();
                    let sender = sender.clone();
                    get_handler(sender, msg)
                })
                .collect(),
        }
    }
}

impl<M: Send + 'static> WritingHandler<M> {
    /// Returns an empty handler with no tasks.
    /// Calling `wait` on this handler returns immediately with success.
    pub fn empty() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Waits for all tasks in the handler to finish.
    /// If `duration` is `None`, this method waits indefinitely.
    /// If `duration` is `Some`, it waits only for the given time.
    /// Returns the number of completed tasks on success or a vector of caught errors.
    pub async fn wait(self, duration: Option<Duration>) -> Result<usize, DispatchError<M>> {
        let n = self.handlers.len();
        let mut errors = Vec::new();

        for handler in self.handlers {
            let result = match duration {
                Some(duration) => timeout(duration, handler).await,
                None => Ok(handler.await),
            };

            match result {
                Ok(Ok(Ok(()))) => (),
                Ok(Ok(Err(e))) => errors.push(DispatchError::SendingError(e)),
                Ok(Err(e)) => errors.push(DispatchError::JoiningError(e)),
                Err(_) => errors.push(DispatchError::WritingTimeout(match duration {
                    Some(d) => d,
                    None => {
                        return Err(DispatchError::UnexpectedError(
                            UnexpectedErrorKind::DurationIsMissing, // Should never append as if duration is None we put the result in Ok
                        ));
                    }
                })),
            }
        }

        if errors.is_empty() {
            Ok(n)
        } else {
            Err(DispatchError::WritingSendError(errors))
        }
    }
}
