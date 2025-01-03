use super::task_pool::OpId;
use tokio::sync::mpsc::Sender;

pub trait TaskTrait<Arg, Message, Output> {
    fn begin(arg: Arg, output_sender: Sender<(OpId, Output)>) -> Sender<Message>;
}
