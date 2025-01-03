mod macros;

use std::sync::Arc;

use tokio::sync::Mutex;

pub type Wrapped<T> = Arc<Mutex<T>>;
