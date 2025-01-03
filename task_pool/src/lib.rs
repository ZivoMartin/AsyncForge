mod task;
mod task_pool;

pub use task::TaskTrait;
pub use task_pool::{default_error_handler, OpId, PoolError, TaskPool};
