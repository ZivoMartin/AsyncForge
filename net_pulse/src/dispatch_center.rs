use std::{collections::HashMap, hash::Hash, sync::Arc};

use crate::{
    error::{DispatchError, UnexpectedErrorKind},
    get_senders,
    writing_handler::WritingHandler,
    Receiver, Sender,
};
use smart_channel::channel;

// TODO LATER: Create a wrapper behind a mutex for the `DispatchCenter`.
// TODO LATER: Implement methods for message types that implement `Closable` (should include `shutdown_channel` to close a specific channel and `shutdown_all` to close all channels).
// TODO LATER: Implement `Drop` for receivers to enable automatic unsubscription. This requires creating a wrapper containing a shared reference to the `DispatchCenter` to call `unsubscribe`.
// TODO LATER: Add a wait-for-closing notifier. This should behave similarly to the creation waiters, but for destruction events.
// TODO LATER: Add the ability to create `WritingHandler` directly from user input.
// TODO LATER: Add the possibility to choose specific tasks to wait for in `WritingHandler`.
// TODO LATER: Improve error handling in `WritingHandler` by allowing identification of which sender failed.
// TODO LATER: Allow the user to pass this as an argument to certain functions (not yet implemented).

/// The default size of a message channel.
pub(crate) const CHANNEL_SIZE: usize = 100;
/// The default size of a notification channel.
pub(crate) const NOTIFIER_CHANNEL_SIZE: usize = 10;

/// Represents the state of a channel. You can retrieve it by calling `channel_state` on the `DispatchCenter`.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum ChannelState {
    /// The initial state of the channel—no subscribers have ever connected.
    Uninitialised,
    /// The channel has active subscribers. This state remains while there is some subscriber, even if they are not active
    Running,
    /// The channel had subscribers in the past, but they have unsubscribed, or they had dropped and then clean_channel has been called
    Over,
}

/// `SmartChannelId` is a unique identifier for channels within a `DispatchCenter`.
/// It consists of a monotonically increasing counter and the memory address of the `DispatchCenter`
/// (converted to `usize`). This guarantees that the ID is unique across different contexts.
///
/// The address represents a specific field of a specific `DispatchCenter`, ensuring its global uniqueness.
/// We store the address as a `usize` instead of a raw pointer to simplify the type and to keep this type simple without involving generics.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct SmartChannelId {
    /// A counter that increments with each created channel to ensure uniqueness.
    pub(crate) channel_counter: usize,
    /// The memory address of the `DispatchCenter`, stored as a `usize` for simplicity (used as an identifier, not as a dereferenceable address).
    pub(crate) dispatch_center_address: usize,
}

pub type MessageSender<M> = Sender<M, SmartChannelId>;
pub type MessageReceiver<M> = Receiver<M, SmartChannelId>;

pub type Waiter = Receiver<(), SmartChannelId>;
pub type WaiterSender = Sender<(), SmartChannelId>;

/// The main data structure of the crate. It contains all the senders for subscribers and the waiters for channel creation notifications.
/// The `ChannelId` is used to identify differents channels it can be any type as long as it implements Eq, Hash, et for the majority of the functions Clone
pub struct DispatchCenter<M, ChannelId: Eq + Hash> {
    connection_id: usize,
    senders: HashMap<ChannelId, Vec<MessageSender<M>>>,
    waiter_senders: HashMap<ChannelId, Vec<WaiterSender>>,
}

impl<M, ChannelId: Eq + Hash> Default for DispatchCenter<M, ChannelId> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M, ChannelId: Eq + Hash> DispatchCenter<M, ChannelId> {
    /// Returns an empty `DispatchCenter`.
    pub fn new() -> Self {
        DispatchCenter {
            connection_id: 0,
            senders: HashMap::new(),
            waiter_senders: HashMap::new(),
        }
    }

    /// Generates a new unique `SmartChannelId` by incrementing the internal counter and associating it with the memory address of the `DispatchCenter`.
    fn get_new_id(&mut self) -> SmartChannelId {
        let channel_counter = self.connection_id;
        self.connection_id += 1;
        SmartChannelId {
            dispatch_center_address: (self as *const DispatchCenter<M, ChannelId>) as usize,
            channel_counter,
        }
    }

    /// Sends a notification to all waiters subscribed to a channel after a sender is created.
    /// This function should only be called after a sender is added. Since notifications use the unit type `()`,
    /// `new_cloning_broadcast` is used to broadcast to all waiters.
    fn notify_creation(&mut self, id: &ChannelId) -> WritingHandler<()> {
        if let Some(waiters) = self.waiter_senders.get(id) {
            WritingHandler::new_cloning_broadcast(&(), waiters)
        } else {
            WritingHandler::empty()
        }
    }

    /// Returns `true` if the given receiver is subscribed to the specified channel.
    pub fn is_subscribed(&self, channel: &ChannelId, receiver: &MessageReceiver<M>) -> bool {
        match self.channel_state(channel) {
            ChannelState::Running => get_senders!(self, channel)
                .iter()
                .any(|s| s.is_bound_to(receiver)),
            _ => false,
        }
    }

    /// Returns the number of waiters for a given channel.
    pub fn number_of_waiter(&self, id: &ChannelId) -> usize {
        match self.waiter_senders.get(id) {
            Some(w) => w.len(),
            None => 0,
        }
    }

    /// Returns the current state of the specified channel.
    pub fn channel_state(&self, id: &ChannelId) -> ChannelState {
        match self.senders.get(id) {
            Some(s) if !s.is_empty() => ChannelState::Running,
            Some(_) => ChannelState::Over,
            None => ChannelState::Uninitialised,
        }
    }

    /// Returns the number of subscribers for a specific channel. Returns `0` if the channel is uninitialised or has ended.
    pub fn channel_number_subscriber(&self, id: &ChannelId) -> usize {
        match self.channel_state(id) {
            ChannelState::Over | ChannelState::Uninitialised => 0,
            ChannelState::Running => get_senders!(self, id).len(),
        }
    }

    /// Cleans up closed connections by removing senders that are closed. Returns the new state of the channel after cleaning.
    pub fn clean_channel(&mut self, channel: &ChannelId) -> ChannelState {
        let senders = match self.senders.get_mut(channel) {
            Some(s) => s,
            None => return ChannelState::Uninitialised,
        };
        senders.retain(|s| !s.is_closed());
        if senders.is_empty() {
            ChannelState::Over
        } else {
            ChannelState::Running
        }
    }
}

impl<M, ChannelId> DispatchCenter<Arc<M>, ChannelId>
where
    M: Send + Sync + 'static,
    ChannelId: Eq + Hash + Clone,
{
    /// Sends an `Arc`-wrapped message to all channels. Cleans inactive receivers before sending.
    /// Useful for broadcasting large messages without cloning the data.
    pub fn broadcast_arc(&mut self, msg: M) -> WritingHandler<Arc<M>> {
        self.clean_all();
        let senders: Vec<_> = self
            .senders
            .values()
            .flat_map(|s| s.iter().cloned())
            .collect();
        WritingHandler::new_arc_broadcast(msg, &senders)
    }

    /// Sends a shared reference of the given message to subscribers.
    /// Returns an error if the channel is uninitialised, and an empty handler if it has ended.
    /// Cleans inactive receivers before sending.
    pub fn arc_send(
        &mut self,
        msg: M,
        id: &ChannelId,
    ) -> Result<WritingHandler<Arc<M>>, DispatchError<Arc<M>, ChannelId>> {
        self.clean_channel(&id);
        match self.channel_state(&id) {
            ChannelState::Running => Ok(WritingHandler::new_arc_broadcast(
                msg,
                get_senders!(self, id),
            )),
            ChannelState::Over => Ok(WritingHandler::empty()),
            ChannelState::Uninitialised => Err(DispatchError::ChannelUninitialized(id.clone())),
        }
    }
}

impl<M, ChannelId> DispatchCenter<M, ChannelId>
where
    M: Send + Clone + 'static,
    ChannelId: Eq + Hash + Clone,
{
    /// Broadcasts the cloned message to all channels.
    pub fn broadcast_clone(&mut self, msg: &M) -> WritingHandler<M> {
        self.clean_all();
        let senders: Vec<_> = self
            .senders
            .values()
            .flat_map(|s| s.iter().cloned())
            .collect();
        WritingHandler::new_cloning_broadcast(&msg, &senders)
    }

    /// Sends a cloned value to subscribers. Use this for small, cheap-to-clone messages.
    /// Cleans inactive receivers before sending.
    pub fn clone_send(
        &mut self,
        msg: &M,
        id: &ChannelId,
    ) -> Result<WritingHandler<M>, DispatchError<M, ChannelId>> {
        self.clean_channel(&id);
        match self.channel_state(&id) {
            ChannelState::Running => Ok(WritingHandler::new_cloning_broadcast(
                msg,
                get_senders!(self, id),
            )),
            ChannelState::Over => Ok(WritingHandler::empty()),
            ChannelState::Uninitialised => Err(DispatchError::ChannelUninitialized(id.clone())),
        }
    }
}

impl<M, ChannelId: Eq + Hash + Clone> DispatchCenter<M, ChannelId> {
    /// This function call the clean_channel method for all the initialized channels. Returns an hashmap binding each channel with its new state
    pub fn clean_all(&mut self) -> HashMap<ChannelId, ChannelState> {
        let mut map = HashMap::with_capacity(self.senders.len());
        for id in self.senders.keys().map(|k| k.clone()).collect::<Vec<_>>() {
            map.insert(id.clone(), self.clean_channel(&id));
        }
        map
    }

    /// This functions returns a received subscribed to th the channels given in parameter. If the channel is uninitialised, it insert the sender with the insert sender function
    pub fn subscribe(&mut self, id: &ChannelId) -> MessageReceiver<M> {
        let (sender, receiver) = channel(CHANNEL_SIZE, self.get_new_id());
        self.insert_sender(sender, id);
        receiver
    }

    /// This function insert the sender in the sender and call notify creation to notify the waiter of the channel creation
    /// It writing handler of the notify creation is ignored for now as i don't really now if it is a good idea to returns
    /// it as it would imply to returns a tupple instead of just the single receiver for the subscribe methods.
    fn insert_sender(&mut self, sender: MessageSender<M>, id: &ChannelId) {
        match self.senders.get_mut(id) {
            Some(senders) => senders.push(sender),
            None => {
                self.senders.insert(id.clone(), vec![sender]);
            }
        }
        // Maybe we should wait it here ?
        let _ = self.notify_creation(id);
    }

    /// This functions takes in parameter a receiver and returns all the channels in which the receiver is subscribed.
    pub fn subscribed_list(&self, receiver: &MessageReceiver<M>) -> Vec<ChannelId> {
        self.senders
            .keys()
            .filter(|id| self.is_subscribed(id, receiver))
            .map(|id| id.clone())
            .collect()
    }

    /// Unsubscribes from all subscriptions for the given receiver across all channels.
    /// This function calls `unsubscribe_multiple` using the list returned by `subscribed_list`.
    /// If the receiver is subscribed to multiple channels, it removes the subscriptions for all of them.
    /// Returns the list of channel IDs from which the receiver was unsubscribed.
    pub fn unsubscribe_all(&mut self, receiver: &MessageReceiver<M>) -> Vec<ChannelId> {
        let sub_list = self.subscribed_list(receiver);
        if !sub_list.is_empty() {
            let _ = self.unsubscribe_multiple(&sub_list, receiver); // This should not fail as `subscribed_list` returns only valid channels.
        }
        sub_list
    }

    /// This function takes in parameter a receiver, and remove the associated sender in the given channel, it it exists, otherwise it returns an error. Returns the new state of the channel.
    pub fn unsubscribe(
        &mut self,
        id: &ChannelId,
        receiver: &MessageReceiver<M>,
    ) -> Result<ChannelState, DispatchError<M, ChannelId>> {
        match self.channel_state(id) {
            ChannelState::Running => {
                if !self.is_subscribed(id, receiver) {
                    return Err(DispatchError::NotSubscribed(id.clone()));
                }
                match self.senders.get_mut(id) {
                    Some(senders) => {
                        senders.retain(|sender| !sender.is_bound_to(receiver));
                        Ok(self.channel_state(id))
                    }
                    None => Err(DispatchError::UnexpectedError(
                        UnexpectedErrorKind::InvalidChannelStateUnsubscribe,
                    )), // Should never append as we already checked the state
                }
            }
            _ => Err(DispatchError::NotSubscribed(id.clone())),
        }
    }

    /// This function try to call unsubscribe with all the given ids.
    /// If it fails to unsubribe for one or more of the given ids with the given receiver
    /// the function returns a the NotSubscribeMultiple error which contains all the errors
    /// Note that anyway, all the channels will be unsubscribed at the end of the function even if cath
    /// an error during the process
    pub fn unsubscribe_multiple(
        &mut self,
        ids: &[ChannelId],
        receiver: &MessageReceiver<M>,
    ) -> Result<(), DispatchError<M, ChannelId>> {
        let mut errors = Vec::new();
        for id in ids {
            if let Err(e) = self.unsubscribe(id, receiver) {
                errors.push(e)
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(DispatchError::NotSubscribedMultiple(errors))
        }
    }

    /// This function returns a waiter for the channel. The waiter is notified each time someone subscribe to the channel
    pub fn get_waiter(&mut self, id: &ChannelId) -> Waiter {
        let (sender, receiver) = channel(NOTIFIER_CHANNEL_SIZE, self.get_new_id());
        match self.waiter_senders.get_mut(id) {
            Some(w) => w.push(sender),
            None => {
                self.waiter_senders.insert(id.clone(), vec![sender]);
            }
        }
        receiver
    }
}

impl<M: Clone, ChannelId: Eq + Hash + Clone> DispatchCenter<M, ChannelId> {
    /// Subscribes to all the channels specified in the `ids` array by inserting the same sender into each channel.
    /// A single receiver is returned, bound to all channels.
    /// Since the sender is cloned for each channel, `M` must implement `Clone`.
    pub fn subscribe_multiple(&mut self, ids: &[ChannelId]) -> MessageReceiver<M> {
        let (sender, receiver) = channel(CHANNEL_SIZE, self.get_new_id());
        for id in ids {
            self.insert_sender(sender.clone(), id);
        }
        receiver
    }

    /// Returns the sender associated with a given `receiver` for the specified `channel`, if it exists.
    /// Returns `None` if no matching sender is found.
    /// Since the returned sender is cloned, `M` must implement `Clone`.
    pub fn get_sender(
        &self,
        channel: &ChannelId,
        receiver: &MessageReceiver<M>,
    ) -> Option<MessageSender<M>> {
        self.senders
            .get(channel)
            .and_then(|senders| senders.iter().find(|s| s.is_bound_to(receiver)).cloned())
    }

    /// Returns a map of channels and their corresponding senders associated with the specified `receiver`.
    /// This function checks multiple channels and returns a `HashMap` binding each `ChannelId` to its corresponding `MessageSender`.
    /// Since this function internally calls `get_sender`, `M` must implement `Clone`.
    pub fn get_senders(
        &self,
        receiver: &MessageReceiver<M>,
        channel: &[ChannelId],
    ) -> HashMap<ChannelId, MessageSender<M>> {
        channel
            .iter()
            .filter_map(|id| {
                self.get_sender(id, receiver)
                    .map(|sender| (id.clone(), sender))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch_center::ChannelState;
    use smart_channel::channel;

    #[tokio::test]
    async fn test_empty_dispatch_center() {
        let center: DispatchCenter<String, &'static str> = DispatchCenter::new();

        assert_eq!(
            center.channel_state(&"channel1"),
            ChannelState::Uninitialised
        );
        assert_eq!(center.channel_number_subscriber(&"channel1"), 0);
        assert_eq!(center.number_of_waiter(&"channel1"), 0);
    }

    #[tokio::test]
    async fn test_unique_channel_ids() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let id1 = center.get_new_id();
        let id2 = center.get_new_id();
        let id3 = center.get_new_id();
        assert_ne!(id1, id2);
        assert_ne!(id1, id3);
        assert_ne!(id2, id3);
    }

    #[tokio::test]
    async fn test_is_subscribed() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let (sender, receiver) = channel(10, center.get_new_id());

        center.senders.insert("channel1", vec![sender.clone()]);
        assert!(center.is_subscribed(&"channel1", &receiver));
    }

    #[tokio::test]
    async fn test_channel_number_subscriber() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let (sender1, _receiver1) = channel(10, center.get_new_id());
        let (sender2, _receiver2) = channel(10, center.get_new_id());

        center.senders.insert("channel1", vec![sender1, sender2]);
        assert_eq!(center.channel_number_subscriber(&"channel1"), 2);
    }

    #[tokio::test]
    async fn test_notify_creation() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let (waiter_sender, mut waiter_receiver) = channel(10, center.get_new_id());

        center
            .waiter_senders
            .insert("channel1", vec![waiter_sender]);
        let handler = center.notify_creation(&"channel1");
        let result = handler.wait(None).await;

        assert!(result.is_ok());
        assert!(waiter_receiver.recv().await.is_some()); // Ensure notification was sent.
    }

    #[tokio::test]
    async fn test_number_of_waiter() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let (waiter1, _) = channel(10, center.get_new_id());
        let (waiter2, _) = channel(10, center.get_new_id());

        center
            .waiter_senders
            .insert("channel1", vec![waiter1, waiter2]);
        assert_eq!(center.number_of_waiter(&"channel1"), 2);
    }

    #[tokio::test]
    async fn test_channel_state_transitions() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();

        assert_eq!(
            center.channel_state(&"channel1"),
            ChannelState::Uninitialised
        );

        let (sender, _receiver) = channel(10, center.get_new_id());
        center.senders.insert("channel1", vec![sender]);
        assert_eq!(center.channel_state(&"channel1"), ChannelState::Running);

        center.clean_channel(&"channel1"); // No receivers closed.
        assert_eq!(center.channel_state(&"channel1"), ChannelState::Running);

        center.senders.get_mut("channel1").unwrap().clear(); // Clear all senders.
        assert_eq!(center.channel_state(&"channel1"), ChannelState::Over);
    }

    #[tokio::test]
    async fn test_clean_channel() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let (sender, _) = channel(10, center.get_new_id());

        center.senders.insert("channel1", vec![sender]);
        assert_eq!(center.channel_state(&"channel1"), ChannelState::Running);

        center.clean_channel(&"channel1"); // Clean closed connections.
        assert_eq!(center.channel_state(&"channel1"), ChannelState::Over); // No active senders remain.
    }

    #[tokio::test]
    async fn test_clean_all() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let (sender1, _) = channel(10, center.get_new_id());
        let (sender2, _receiver2) = channel(10, center.get_new_id());

        center.senders.insert("channel1", vec![sender1.clone()]);
        center.senders.insert("channel2", vec![sender2.clone()]);
        assert_eq!(center.channel_state(&"channel1"), ChannelState::Running);
        assert_eq!(center.channel_state(&"channel2"), ChannelState::Running);

        let cleaned_states = center.clean_all();
        assert_eq!(cleaned_states.get(&"channel1"), Some(&ChannelState::Over));
        assert_eq!(
            cleaned_states.get(&"channel2"),
            Some(&ChannelState::Running)
        );
    }

    #[tokio::test]
    async fn test_subscribe() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();

        let (waiter, mut wait_receiver) = channel(10, center.get_new_id());

        center.waiter_senders.insert("channel1", vec![waiter]);

        let receiver = center.subscribe(&"channel1");

        assert_eq!(center.channel_state(&"channel1"), ChannelState::Running);
        assert!(center.is_subscribed(&"channel1", &receiver));
        assert!(center.channel_number_subscriber(&"channel1") == 1);
        assert!(wait_receiver.recv().await == Some(()))
    }

    #[tokio::test]
    async fn test_subscribed_list() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let receiver = center.subscribe(&"channel1");
        center.subscribe(&"channel2");

        let subscribed_channels = center.subscribed_list(&receiver);
        assert!(subscribed_channels == vec!("channel1"));
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let receiver = center.subscribe(&"channel1");

        let result = center.unsubscribe(&"channel1", &receiver);
        assert!(result.is_ok());
        assert_eq!(center.channel_state(&"channel1"), ChannelState::Over);

        let invalid_result = center.unsubscribe(&"channel1", &receiver);
        assert!(matches!(
            invalid_result,
            Err(DispatchError::NotSubscribed("channel1"))
        ));
    }

    #[tokio::test]
    async fn test_unsubscribe_multiple() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let receiver = center.subscribe(&"channel1");
        center.subscribe(&"channel2");

        let result = center.unsubscribe_multiple(&["channel1", "channel2"], &receiver);
        match result {
            Ok(()) => panic!(),
            Err(DispatchError::NotSubscribedMultiple(errors)) => assert!(
                errors.len() == 1 && matches!(errors[0], DispatchError::NotSubscribed("channel2"))
            ),
            _ => panic!("Unexpected error"),
        }

        assert!(!center.is_subscribed(&"channel1", &receiver));
        assert_eq!(center.channel_state(&"channel1"), ChannelState::Over);
        assert_eq!(center.channel_state(&"channel2"), ChannelState::Running);
    }

    #[tokio::test]
    async fn test_get_waiter() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let mut waiter = center.get_waiter(&"channel1");

        let _ = center.subscribe(&"channel1");
        assert!(waiter.recv().await.is_some());
    }

    #[tokio::test]
    async fn test_subscribe_multiple() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let receiver = center.subscribe_multiple(&["channel1", "channel2"]);

        assert!(center.is_subscribed(&"channel1", &receiver));
        assert!(center.is_subscribed(&"channel2", &receiver));
    }

    #[tokio::test]
    async fn test_get_sender() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let receiver = center.subscribe(&"channel1");

        let sender = center.get_sender(&"channel1", &receiver);
        assert!(sender.is_some());

        let nonexistent_sender = center.get_sender(&"channel2", &receiver);
        assert!(nonexistent_sender.is_none());
    }

    #[tokio::test]
    async fn test_get_senders() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let receiver = center.subscribe_multiple(&["channel1", "channel2"]);

        let senders = center.get_senders(&receiver, &["channel1", "channel2"]);
        assert_eq!(senders.len(), 2);
        assert!(senders.contains_key(&"channel1"));
        assert!(senders.contains_key(&"channel2"));

        let empty_senders = center.get_senders(&receiver, &["channel3", "channel1"]);
        assert!(empty_senders.len() == 1);
    }

    #[tokio::test]
    async fn test_unsubscribe_all_multiple_channels() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let _receiver1 = center.subscribe(&"channel1");
        let _receiver2 = center.subscribe(&"channel2");
        let _receiver3 = center.subscribe(&"channel3");

        let receiver = center.subscribe_multiple(&["channel1", "channel2", "channel3"]);

        let unsubscribed_channels = center.unsubscribe_all(&receiver);
        assert_eq!(unsubscribed_channels.len(), 3);
        assert!(!center.is_subscribed(&"channel1", &receiver));
        assert!(!center.is_subscribed(&"channel2", &receiver));
        assert!(!center.is_subscribed(&"channel3", &receiver));
    }

    #[tokio::test]
    async fn test_broadcast_arc() {
        let mut center: DispatchCenter<Arc<String>, &'static str> = DispatchCenter::new();
        let receiver1 = center.subscribe_multiple(&["channel1", &"channel2"]);
        let _receiver2 = center.subscribe(&"channel3");

        let msg = "Hello ARC broadcast!".to_string();
        let handler = center.broadcast_arc(msg.clone());

        assert_eq!(handler.len(), 3);

        center.unsubscribe_all(&receiver1);

        let handler_after_drop = center.broadcast_arc(msg.clone());
        assert_eq!(handler_after_drop.len(), 1);
    }

    #[tokio::test]
    async fn test_arc_send() {
        let mut center: DispatchCenter<Arc<String>, &'static str> = DispatchCenter::new();
        let receiver = center.subscribe(&"channel1");

        let msg = "Hello ARC send!".to_string();
        let handlers = center.arc_send(msg, &"channel1").unwrap();
        assert_eq!(handlers.len(), 1);

        // Test uninitialised channel
        let uninitialised_result =
            center.arc_send("Message to no channel".to_string(), &"channel2");

        assert!(matches!(
            uninitialised_result,
            Err(DispatchError::ChannelUninitialized("channel2"))
        ));

        center.unsubscribe(&"channel1", &receiver).unwrap();

        // Close the channel and test
        center.clean_channel(&"channel1");
        let closed_result = center.arc_send("Message to nobody".to_string(), &"channel1");
        assert_eq!(closed_result.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_clone_send() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let receiver = center.subscribe(&"channel1");

        let msg = "Clone send message".to_string();
        let result = center.clone_send(&msg, &"channel1");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);

        // Test uninitialised channel
        let uninitialised_result = center.clone_send(&"No such channel".to_string(), &"channel2");
        assert!(matches!(
            uninitialised_result,
            Err(DispatchError::ChannelUninitialized("channel2"))
        ));

        center.unsubscribe(&"channel1", &receiver).unwrap();

        // Test closed channel
        center.clean_channel(&"channel1");
        let closed_result = center.clone_send(&msg, &"channel1");
        assert_eq!(closed_result.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_broadcast_clone() {
        let mut center: DispatchCenter<String, &'static str> = DispatchCenter::new();
        let mut receiver1 = center.subscribe(&"channel1");
        let mut receiver2 = center.subscribe(&"channel2");

        let msg = "Clone broadcast message".to_string();
        let handler = center.broadcast_clone(&msg);
        assert_eq!(handler.len(), 2); // Two channels

        assert_eq!(
            receiver1.recv().await.unwrap(),
            "Clone broadcast message".to_string()
        );

        assert_eq!(
            receiver2.recv().await.unwrap(),
            "Clone broadcast message".to_string()
        );

        // Drop receivers and broadcast again
        drop(receiver1);
        drop(receiver2);

        let handler_after_drop = center.broadcast_clone(&msg);
        assert_eq!(handler_after_drop.len(), 0); // No active receivers
    }
}
