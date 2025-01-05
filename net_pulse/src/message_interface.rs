pub trait SendableMessage: Send + 'static + Sync {
    const NB_SENDERS: usize;

    fn to_id(&self) -> &'static str;
    fn is_close(&self) -> bool;
    fn close() -> Self;
}
