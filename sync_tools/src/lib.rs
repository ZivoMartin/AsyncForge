mod macros;

use std::sync::Arc;

use tokio::sync::RwLock;

pub type Wrapped<T> = Arc<RwLock<T>>;
