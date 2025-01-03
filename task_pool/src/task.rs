use crate::{
    errors::{PoolError, PoolResult},
    task_pool::OpId,
    Sender,
};

type TaskOutputSender<Output> = Sender<(OpId, Output)>;

/// This struct is given to each task at the begining of it
pub struct TaskInterface<Output> {
    pub(crate) id: OpId,
    pub(crate) output_sender: TaskOutputSender<Output>,
}

impl<Output> TaskInterface<Output> {
    /// Returns the id of the task
    pub fn id(&self) -> OpId {
        self.id
    }

    /// Send the output to the pool via a channel, returns an error if the sender fail to handle the message
    pub async fn output(&self, output: Output) -> PoolResult<()> {
        if self.output_sender.send((self.id, output)).await.is_err() {
            Err(PoolError::FailedToOutput(self.id))
        } else {
            Ok(())
        }
    }
}

/// To use a task pool you should implement this trait to your tasks.
pub trait TaskTrait<Arg, Message, Output> {
    /// The begin method takes in parameter an arg of any type that you can pass to your task, and the task interface
    fn begin(arg: Arg, output_sender: TaskInterface<Output>) -> Sender<Message>;
}
