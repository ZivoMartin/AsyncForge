use crate::task::TaskTrait;
use std::collections::HashMap;
use std::marker::Send;
use sync_tools::{wrap, Wrapped};
use tokio::sync::mpsc::{channel, Receiver, Sender};

use thiserror::Error;

pub type OpId = u64;

#[derive(Debug, Error)]
pub enum PoolError {
    #[error("Failed to send message to process {0}")]
    SendError(OpId),

    #[error("Failed to receive message")]
    ReceiveError,

    #[error("Task {0} is already closed")]
    TaskClosed(OpId),

    #[error("Task {0} creation failed")]
    TaskCreationError(OpId),

    #[error("Cleaning notifier error")]
    CleaningNotifierError,

    #[error("Task {0} already exists and is running")]
    TaskAlreadyExists(OpId),

    #[error("Task {0} creation sending failed")]
    TaskCreationSendingError(OpId),

    #[error("Failed to wait creation of process {0}")]
    FailedToWaitCreation(OpId),

    #[error("Task {0} doesn't exist")]
    TaskNotExist(OpId),

    #[error("The process pool is fully dropped")]
    End,
}

type PoolResult<T> = Result<T, PoolError>;

#[derive(PartialEq, Eq, Copy, Clone)]
enum TaskState {
    Running,
    Closed,
}

#[derive(Clone)]
pub struct PoolTaskEnded<Output: Send + Clone> {
    pub output: Output,
    pub id: OpId,
    pub has_cleared: bool,
}

/// This is an implementation of a generic process pool . The pool can create processes
/// and send messages to the spawned processes. It also tracks the state of each process
/// and provides a sender for easily communicating messages through a Tokio channel.
///
/// As a generic process pool, it defines a `Task` trait, which includes a `begin` method
/// that returns a sender for message transmission. Messages are generic, but all processes
/// within the pool must handle the same message type, even if the processes themselves differ.
///
/// Additionally, the pool can externally notify about process creation and termination events.
/// When a process generates output, it sends the result back to the pool through a sender,
/// and the pool broadcasts this result to all interested parties.

struct WrappedTaskPool<Message: Send, Output: Send + Clone> {
    pool: HashMap<OpId, Sender<Message>>,
    end_process_sender: Sender<(OpId, Output)>,
    output_result_senders: Vec<Sender<PoolTaskEnded<Output>>>,
    process_creation_senders: HashMap<OpId, Vec<Sender<()>>>,
    process_states: HashMap<OpId, TaskState>,
    cleaning_notifier: Option<(Sender<()>, usize)>,
}

impl<Message: Send + 'static, Output: Default + Send + 'static + Clone>
    WrappedTaskPool<Message, Output>
{
    fn new(error_sender: Sender<PoolError>) -> Wrapped<Self> {
        let (end_process_sender, receiver) = channel(100);
        let pool = wrap!(WrappedTaskPool {
            pool: HashMap::new(),
            end_process_sender,
            output_result_senders: Vec::new(),
            process_creation_senders: HashMap::new(),
            process_states: HashMap::new(),
            cleaning_notifier: None,
        });
        Self::listen_for_ending_process(pool.clone(), receiver, error_sender);
        pool
    }

    fn should_clear(&self) -> bool {
        self.cleaning_notifier.is_some()
            && self.pool.is_empty()
            && self.process_states.len() == self.cleaning_notifier.as_ref().unwrap().1
    }

    /// This function takes an id and remove it from the pool. If the pool is in cleaning stage, then the process is totally removed, otherwise his state will pass in Closed
    /// This function may fail if the process is already over, or if the pool is in cleaning stage with a uninitilised or closed cleaning notifier
    async fn handle_ending_process(&mut self, ending_process: OpId) -> PoolResult<bool> {
        if self.process_states.get(&ending_process) != Some(&TaskState::Running) {
            return Err(PoolError::TaskClosed(ending_process));
        }
        self.pool.remove(&ending_process);

        self.process_states
            .insert(ending_process, TaskState::Closed);
        let should_clear = self.should_clear();
        if should_clear {
            self.process_states.clear();
            self.process_creation_senders.clear();
            let notifier = self.cleaning_notifier.take().unwrap().0; // Can't fail
            if notifier.send(()).await.is_err() {
                return Err(PoolError::CleaningNotifierError);
            }
        }
        Ok(should_clear)
    }

    /// This function is the main loop of the pool, all the endings processes are received here. If the gestion of an ending process failed, then the erreor is given via the error_sender. If error_sender
    /// isn't valid, then the function simply ignore the errors.
    fn listen_for_ending_process(
        pool: Wrapped<Self>,
        mut receiver: Receiver<(OpId, Output)>,
        error_sender: Sender<PoolError>,
    ) {
        tokio::spawn(async move {
            loop {
                let (ending_process, result) = match receiver.recv().await {
                    Some(r) => r,
                    None => {
                        let _ = error_sender.send(PoolError::End).await;
                        return;
                    }
                };
                let mut pool = pool.lock().await;
                match pool.handle_ending_process(ending_process).await {
                    Ok(has_cleared) => {
                        let mut senders_to_remove = Vec::new();
                        for (i, sender) in pool.output_result_senders.iter().enumerate() {
                            if sender
                                .send(PoolTaskEnded {
                                    id: ending_process,
                                    output: result.clone(),
                                    has_cleared,
                                })
                                .await
                                .is_err()
                            {
                                senders_to_remove.push(i)
                            }
                        }

                        for to_remove in senders_to_remove.into_iter().rev() {
                            pool.output_result_senders.remove(to_remove);
                        }
                    }
                    Err(e) => {
                        let _ = error_sender.send(e).await;
                    }
                }
            }
        });
    }

    /// This function creates a new process and inserts it into the pool. This process must implement the TaskTrait which allows the pool to start it with a generic argument via the begin function of the TaskTrait. The function may fail if the process is already running, but will not fail if the process was running but is now closed. If the creation of the process identifier was expected by a process_creation_waiter, a notification is sent to the latter. The function returns an error if one of the sender is invalid.
    async fn new_process<Task: TaskTrait<TaskArg, Message, Output>, TaskArg>(
        &mut self,
        id: OpId,
        config: TaskArg,
    ) -> PoolResult<()> {
        if self.process_states.insert(id, TaskState::Running) == Some(TaskState::Running) {
            return Err(PoolError::TaskAlreadyExists(id));
        }
        let mut result = Ok(());
        if let Some(senders) = self.process_creation_senders.remove(&id) {
            for s in senders {
                if s.send(()).await.is_err() {
                    result = Err(PoolError::TaskCreationSendingError(id));
                };
            }
        }
        let sender = Task::begin(config, self.end_process_sender.clone());
        self.pool.insert(id, sender);
        result
    }

    /// This function waits the creation of a process and send it the generic message passed in argument. The function fail if the process is closed, or if for some reason the communication between thread
    /// fail.
    async fn wait_and_send(pool: Wrapped<Self>, id: OpId, msg: Message) -> PoolResult<()> {
        let mut receiver = {
            let mut pool = pool.lock().await;
            let state = pool.process_states.get(&id);
            match state {
                None => pool.new_process_notif_receiver(id),
                Some(TaskState::Running) => {
                    return match pool.send(id, msg).await {
                        Ok(()) => Ok(()),
                        _ => Err(PoolError::SendError(id)),
                    }
                }
                Some(TaskState::Closed) => return Err(PoolError::TaskClosed(id)),
            }
        };
        if receiver.recv().await.is_none() {
            return Err(PoolError::FailedToWaitCreation(id));
        }
        Ok(())
    }

    async fn send(&self, id: OpId, msg: Message) -> PoolResult<()> {
        match self.pool.get(&id) {
            Some(sender) => {
                if sender.send(msg).await.is_err() {
                    return Err(PoolError::SendError(id));
                };
                Ok(())
            }
            None => Err(PoolError::TaskNotExist(id)),
        }
    }

    fn new_result_redirection(&mut self) -> Receiver<PoolTaskEnded<Output>> {
        let (sender, receiver) = channel(100);
        self.output_result_senders.push(sender);
        receiver
    }

    fn new_process_notif_receiver(&mut self, id: OpId) -> Receiver<()> {
        let (sender, receiver) = channel(100);
        match self.process_creation_senders.get_mut(&id) {
            Some(senders) => senders.push(sender),
            None => {
                let _ = self.process_creation_senders.insert(id, vec![sender]);
            }
        }
        receiver
    }

    fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }

    async fn clean(&mut self, awaited: usize) -> Option<Receiver<()>> {
        let (sender, receiver) = channel(100);
        if self.pool.is_empty() {
            self.process_creation_senders.clear();
            self.process_states.clear();
            None
        } else {
            self.cleaning_notifier = Some((sender, awaited));
            Some(receiver)
        }
    }
}

pub struct TaskPool<Message: Send, Output: Send + Clone + Default> {
    pool: Wrapped<WrappedTaskPool<Message, Output>>,
}

impl<Message: Send + 'static, Output: Send + 'static + Clone + Default> Default
    for TaskPool<Message, Output>
{
    fn default() -> Self {
        let (sender, receiver) = channel(100);
        default_error_handler(receiver);
        Self {
            pool: WrappedTaskPool::new(sender),
        }
    }
}

impl<Message: Send + 'static, Output: Send + 'static + Clone + Default> Clone
    for TaskPool<Message, Output>
{
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
        }
    }
}

impl<Message: Send + 'static, Output: Send + 'static + Clone + Default> TaskPool<Message, Output> {
    pub fn new() -> (Self, Receiver<PoolError>) {
        let (sender, receiver) = channel(100);
        (
            Self {
                pool: WrappedTaskPool::new(sender),
            },
            receiver,
        )
    }

    pub async fn new_process<Task: TaskTrait<TaskArg, Message, Output>, TaskArg>(
        &self,
        id: OpId,
        config: TaskArg,
    ) -> PoolResult<()> {
        self.pool
            .lock()
            .await
            .new_process::<Task, TaskArg>(id, config)
            .await
    }

    pub async fn new_result_redirection(&mut self) -> Receiver<PoolTaskEnded<Output>> {
        self.pool.lock().await.new_result_redirection()
    }

    pub async fn is_empty(&self) -> bool {
        self.pool.lock().await.is_empty()
    }

    pub async fn wait_and_send(&self, id: OpId, msg: Message) -> PoolResult<()> {
        WrappedTaskPool::wait_and_send(self.pool.clone(), id, msg).await
    }

    pub async fn send(&self, id: OpId, msg: Message) -> PoolResult<()> {
        self.pool.lock().await.send(id, msg).await
    }

    pub async fn clean(&self, awaited: usize) -> Option<Receiver<()>> {
        self.pool.lock().await.clean(awaited).await
    }
}

pub fn default_error_handler(mut error_receiver: Receiver<PoolError>) {
    tokio::spawn(async move {
        if let Some(e) = error_receiver.recv().await {
            panic!("{e}")
        }
    });
}
