use crate::{
    channel,
    errors::ErrorReceiver,
    errors::{panic_error_handler, PoolError, PoolResult},
    task::{TaskInterface, TaskTrait},
    Receiver, Sender,
};
use std::marker::Send;
use std::{collections::HashMap, sync::Arc};
use sync_tools::{wrap, Wrapped};
use tracing::error;

pub type OpId = u64;

#[derive(PartialEq, Eq, Copy, Clone)]
enum TaskState {
    Running,
    Closed,
}

/// This struct represents the result of a completed task. It contains the output, the task ID, and a boolean indicating whether the output cleared the pool (true) or not (false).
#[derive(Clone)]
pub struct PoolTaskEnded<Output: Send> {
    pub output: Arc<Output>,
    pub id: OpId,
    pub has_cleared: bool,
}

pub type OutputSender<Output> = Sender<PoolTaskEnded<Output>>;
pub type OutputReceiver<Output> = Receiver<PoolTaskEnded<Output>>;

struct WrappedTaskPool<Message: Send, Output: Send> {
    pool: HashMap<OpId, Sender<Message>>,
    end_task_sender: Sender<(OpId, Output)>,
    output_result_senders: Vec<OutputSender<Output>>,
    task_creation_senders: HashMap<OpId, Vec<Sender<()>>>,
    task_states: HashMap<OpId, TaskState>,
    cleaning_notifier: Option<(Sender<()>, Option<usize>)>,
}

impl<Message: Send + 'static, Output: Send + 'static + Sync> WrappedTaskPool<Message, Output> {
    fn new(error_sender: Sender<PoolError>) -> Wrapped<Self> {
        let (end_task_sender, receiver) = channel(100);
        let pool = wrap!(WrappedTaskPool {
            pool: HashMap::new(),
            end_task_sender,
            output_result_senders: Vec::new(),
            task_creation_senders: HashMap::new(),
            task_states: HashMap::new(),
            cleaning_notifier: None,
        });
        Self::listen_for_ending_task(pool.clone(), receiver, error_sender);
        pool
    }

    fn should_clear(&self) -> bool {
        if let Some((_, awaited)) = self.cleaning_notifier {
            let awaited_satisfied = if let Some(awaited) = awaited {
                self.task_states.len() == awaited
            } else {
                true
            };
            self.cleaning_notifier.is_some() && self.pool.is_empty() && awaited_satisfied
        } else {
            false
        }
    }

    /// Removes a task from the pool based on its ID. If the pool is in the cleaning stage, the task is fully removed; otherwise, its state is set to "Closed".
    /// This function may fail if the task has already ended or if the cleaning notifier is uninitialized or closed.
    async fn handle_ending_task(&mut self, ending_task: OpId) -> PoolResult<bool> {
        if self.task_states.get(&ending_task) != Some(&TaskState::Running) {
            return Err(PoolError::TaskClosed(ending_task));
        }
        self.pool.remove(&ending_task);

        self.task_states.insert(ending_task, TaskState::Closed);
        let should_clear = self.should_clear();
        if should_clear {
            self.task_states.clear();
            self.task_creation_senders.clear();
            let notifier = self.cleaning_notifier.take().unwrap().0; // Can't fail
            if notifier.send(()).await.is_err() {
                return Err(PoolError::CleaningNotifierError);
            }
        }
        Ok(should_clear)
    }

    /// This function represents the main loop of the pool where all completed tasks are handled.
    /// If handling a task fails, the error is sent through the `error_sender`. If `error_sender`
    /// is not valid, the function log the errors.
    fn listen_for_ending_task(
        pool: Wrapped<Self>,
        mut receiver: Receiver<(OpId, Output)>,
        error_sender: Sender<PoolError>,
    ) {
        tokio::spawn(async move {
            loop {
                let (ending_task, result) = match receiver.recv().await {
                    Some(r) => r,
                    None => {
                        return;
                    }
                };
                let mut pool = pool.write().await;
                match pool.handle_ending_task(ending_task).await {
                    Ok(has_cleared) => {
                        let mut senders_to_remove = Vec::new();
                        let result = Arc::new(result);
                        for (i, sender) in pool.output_result_senders.iter().enumerate() {
                            let output = PoolTaskEnded::<Output> {
                                id: ending_task,
                                output: Arc::clone(&result),
                                has_cleared,
                            };
                            if sender.send(output).await.is_err() {
                                senders_to_remove.push(i)
                            }
                        }

                        for to_remove in senders_to_remove.into_iter().rev() {
                            pool.output_result_senders.remove(to_remove);
                        }
                    }
                    Err(e) => {
                        if let Err(e) = error_sender.send(e).await {
                            error!("Failed to send error: {e:?}");
                        }
                    }
                }
            }
        });
    }

    async fn new_task<Task: TaskTrait<TaskArg, Message, Output>, TaskArg>(
        &mut self,
        id: OpId,
        arg: TaskArg,
    ) -> PoolResult<()> {
        if self.task_states.insert(id, TaskState::Running) == Some(TaskState::Running) {
            return Err(PoolError::TaskAlreadyExists(id));
        }
        let mut result = Ok(());
        if let Some(senders) = self.task_creation_senders.remove(&id) {
            for s in senders {
                if s.send(()).await.is_err() {
                    result = Err(PoolError::TaskCreationSendingError(id));
                };
            }
        }
        let sender = Task::begin(
            arg,
            TaskInterface {
                id,
                output_sender: self.end_task_sender.clone(),
            },
        );
        self.pool.insert(id, sender);
        result
    }

    async fn wait_for_task_creation(pool: &Wrapped<Self>, id: OpId) -> PoolResult<()> {
        let mut receiver = {
            let mut pool = pool.write().await;
            let state = pool.task_states.get(&id);
            match state {
                None => pool.new_task_notif_receiver(id),
                Some(TaskState::Running) => return Ok(()),
                Some(TaskState::Closed) => return Err(PoolError::TaskClosed(id)),
            }
        };
        if receiver.recv().await.is_none() {
            return Err(PoolError::FailedToWaitCreation(id));
        }
        Ok(())
    }

    async fn wait_and_send(pool: &Wrapped<Self>, id: OpId, msg: Message) -> PoolResult<()> {
        Self::wait_for_task_creation(pool, id).await?;
        pool.read().await.send(id, msg).await
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

    fn new_result_redirection(&mut self) -> OutputReceiver<Output> {
        let (sender, receiver) = channel(100);
        self.output_result_senders.push(sender);
        receiver
    }

    fn new_task_notif_receiver(&mut self, id: OpId) -> Receiver<()> {
        let (sender, receiver) = channel(100);
        match self.task_creation_senders.get_mut(&id) {
            Some(senders) => senders.push(sender),
            None => {
                let _ = self.task_creation_senders.insert(id, vec![sender]);
            }
        }
        receiver
    }

    fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }

    async fn clean(&mut self, awaited: Option<usize>) -> Option<Receiver<()>> {
        let (sender, receiver) = channel(100);
        if self.pool.is_empty() {
            self.task_creation_senders.clear();
            self.task_states.clear();
            None
        } else {
            self.cleaning_notifier = Some((sender, awaited));
            Some(receiver)
        }
    }
}

/// This is an implementation of a generic task pool . The pool can instantiate task
/// and send messages to the spawned tasks. It also tracks the state of each task
/// and provides a sender for easily communicating messages through a Tokio channel.
///
/// As a generic task pool, it defines a `Task` trait, which includes a `begin` method
/// that returns a sender for message transmission. Messages are generic, but all tasks
/// within the pool must handle the same message type, even if the tasks themselves differ.
///
/// The pool is also able to wait for the creation of a specifiv task, the g
///
/// Additionally, the pool can externally notify about task creation and termination events.
/// When a task generates output, it sends the result back to the pool through a sender,
/// and the pool broadcasts this result to all interested parties.
pub struct TaskPool<Message: Send, Output: Send + Sync> {
    pool: Wrapped<WrappedTaskPool<Message, Output>>,
}

impl<Message: Send + 'static, Output: Send + 'static + Sync> Default for TaskPool<Message, Output> {
    fn default() -> Self {
        let (sender, receiver) = channel(100);
        panic_error_handler(receiver);
        Self {
            pool: WrappedTaskPool::new(sender),
        }
    }
}

impl<Message: Send + 'static, Output: Send + 'static + Sync> Clone for TaskPool<Message, Output> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
        }
    }
}

impl<Message: Send + 'static, Output: Send + 'static + Sync> TaskPool<Message, Output> {
    /// This function creates a pool and return it along with a receiver to handle errors in result reception.
    /// Note that the pool implements Default, the default implemenation uses panic_error_handler to handle errors
    pub fn new() -> (Self, ErrorReceiver) {
        let (sender, receiver) = channel(100);
        (
            Self {
                pool: WrappedTaskPool::new(sender),
            },
            receiver,
        )
    }

    /// Creates a new task and adds it to the pool. The task must implement `TaskTrait`, allowing the pool to start it with a generic argument via the `begin` function.
    /// This function may fail if the task is already running, but not if the task was previously running and is now closed.
    /// If a `task_creation_waiter` is waiting for the task ID, a notification is sent.
    /// Returns an error if one of the senders is invalid.
    pub async fn new_task<Task: TaskTrait<TaskArg, Message, Output>, TaskArg>(
        &self,
        id: OpId,
        config: TaskArg,
    ) -> PoolResult<()> {
        self.pool
            .write()
            .await
            .new_task::<Task, TaskArg>(id, config)
            .await
    }

    /// This function waits the creation of a task and send it the generic message passed in argument. The function fail if the task is closed or if we fail to receiv the creation notification
    pub async fn wait_for_task_creation(&self, id: OpId) -> PoolResult<()> {
        WrappedTaskPool::wait_for_task_creation(&self.pool, id).await
    }

    ///  Returns a receiver of task result, when a task ended the result will be sent through it
    pub async fn new_result_redirection(&self) -> OutputReceiver<Output> {
        self.pool.write().await.new_result_redirection()
    }

    /// Returns true if there is no running process in the pool
    pub async fn is_empty(&self) -> bool {
        self.pool.read().await.is_empty()
    }

    /// Waits for a task to be created and sends a message to it.
    /// Fails if the wait fails or if inter-thread communication encounters an error.
    pub async fn wait_and_send(&self, id: OpId, msg: Message) -> PoolResult<()> {
        WrappedTaskPool::wait_and_send(&self.pool, id, msg).await
    }

    /// This function takes a message and an id and try to send via the task sender the message to the running task. The function may return an error if the task does not exist or is over, or if the sender fail to give the message.
    pub async fn send(&self, id: OpId, msg: Message) -> PoolResult<()> {
        self.pool.read().await.send(id, msg).await
    }

    /// This function put the pool in cleaning phase, the awaited parameter represents the number of process that have to be awaited, if None we simply wait for the pool to be empty. If the pool is already cleaned, then the function returns None directly, otherwise, the function bring the pool in cleaning phase and returns a receiver that will notify when the pool is cleaned
    pub async fn clean(&self, awaited: Option<usize>) -> Option<Receiver<()>> {
        self.pool.write().await.clean(awaited).await
    }
}

#[tokio::test]
async fn test_simple_task_execution() {
    let (task_pool, _) = TaskPool::<String, String>::new();

    struct EchoTask;

    impl TaskTrait<String, String, String> for EchoTask {
        fn begin(_: String, task_interface: TaskInterface<String>) -> Sender<String> {
            let (sender, mut receiver) = channel(1);
            tokio::spawn(async move {
                while let Some(input) = receiver.recv().await {
                    task_interface
                        .output(format!("Echo: {input}"))
                        .await
                        .unwrap();
                }
            });
            sender
        }
    }

    let task_id = 1;
    task_pool
        .new_task::<EchoTask, _>(task_id, "Hello".to_string())
        .await
        .unwrap();
    task_pool
        .send(task_id, "Hello again!".to_string())
        .await
        .unwrap();

    let mut result_receiver = task_pool.new_result_redirection().await;
    let result = result_receiver.recv().await.unwrap();
    assert_eq!(result.output.as_ref(), "Echo: Hello again!");
}

#[tokio::test]
async fn test_task_does_not_exist() {
    let (task_pool, _) = TaskPool::<String, String>::new();
    let task_id = 999;
    let result = task_pool.send(task_id, "Invalid task".to_string()).await;

    assert!(matches!(result, Err(PoolError::TaskNotExist(_))));
}

#[tokio::test]
async fn test_pool_cleaning() {
    let (task_pool, _) = TaskPool::<String, String>::new();

    struct DummyTask;
    impl TaskTrait<(), String, String> for DummyTask {
        fn begin(_: (), task_interface: TaskInterface<String>) -> Sender<String> {
            let (sender, _) = channel(1);
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                let _ = task_interface.output("Completed".to_string()).await;
            });
            sender
        }
    }

    let none_cleaner = task_pool.clean(None).await;
    assert!(none_cleaner.is_none());

    let awaited = 200;
    for i in 0..awaited {
        task_pool.new_task::<DummyTask, _>(i, ()).await.unwrap();
    }

    let cleaner = task_pool.clean(Some(awaited as usize)).await;
    assert!(cleaner.is_some());
    cleaner.unwrap().recv().await;

    let none_cleaner = task_pool.clean(None).await;
    assert!(none_cleaner.is_none());
}

#[tokio::test]
async fn test_send_concurrent_tasks() {
    let (task_pool, _) = TaskPool::<u64, u64>::new();

    struct IncrementTask;
    impl TaskTrait<u64, u64, u64> for IncrementTask {
        fn begin(init_val: u64, task_interface: TaskInterface<u64>) -> Sender<u64> {
            let (sender, mut receiver) = channel(1);
            tokio::spawn(async move {
                while let Some(val) = receiver.recv().await {
                    task_interface.output(init_val + val).await.unwrap();
                }
            });
            sender
        }
    }

    for i in 0..5 {
        task_pool.new_task::<IncrementTask, _>(i, i).await.unwrap();
        task_pool.send(i, 10).await.unwrap();
    }

    let mut result_receiver = task_pool.new_result_redirection().await;
    for _ in 0..5 {
        let result = result_receiver.recv().await.unwrap();
        assert_eq!(result.output.as_ref(), &(result.id + 10));
    }
}

#[tokio::test]
async fn test_wait_and_send_concurrent_tasks() {
    let (task_pool, _) = TaskPool::<u64, u64>::new();

    struct IncrementTask;
    impl TaskTrait<u64, u64, u64> for IncrementTask {
        fn begin(init_val: u64, task_interface: TaskInterface<u64>) -> Sender<u64> {
            let (sender, mut receiver) = channel(1);
            tokio::spawn(async move {
                while let Some(val) = receiver.recv().await {
                    task_interface.output(init_val + val).await.unwrap();
                }
            });
            sender
        }
    }

    for i in 0..5 {
        let task_pool = task_pool.clone();
        tokio::spawn(async move { task_pool.wait_and_send(i, 10).await.unwrap() });
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    for i in 0..5 {
        task_pool.new_task::<IncrementTask, _>(i, i).await.unwrap();
    }

    let mut result_receiver = task_pool.new_result_redirection().await;

    for _ in 0..5 {
        let result = result_receiver.recv().await.unwrap();
        assert_eq!(result.output.as_ref(), &(result.id + 10));
    }
}
