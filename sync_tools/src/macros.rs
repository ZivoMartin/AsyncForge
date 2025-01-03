#[macro_export]
macro_rules! wrap {
    ($name:expr) => {
        std::sync::Arc::new(tokio::sync::Mutex::new($name))
    };
}
