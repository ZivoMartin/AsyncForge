/// Get the senders of a given channel and returns a pointer to an empty vec if uninitialised. First case returns immutable.
#[macro_export]
macro_rules! get_senders {
    ($center:expr, $id:expr) => {
        $center.senders.get($id).unwrap_or(&Vec::new())
    };
}
