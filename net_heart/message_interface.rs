pub trait SendableMessage: Clone + Send + 'static {
    const NB_SENDERS: usize;

    fn str_to_id(the_s: &str) -> usize;
    fn get_id(&self) -> usize;
    fn to_str(&self) -> &'static str;
    fn is_close(&self) -> bool;
    fn close() -> Self;
}
