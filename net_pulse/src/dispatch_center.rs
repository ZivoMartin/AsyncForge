use std::{collections::HashMap, sync::Arc};

use crate::{
    error::{DispatchError, UnexpectedErrorKind},
    get_senders,
    message_interface::SendableMessage,
    writing_future::WritingHandler,
    Receiver, Sender,
};
use smart_channel::channel;

// TODO: Implemenater les methodes en todo
// TODO: Finir la gestion d'erreure
// TODO: Passer id en generic
// TODO: Faire un wrapper derriere un mutex pour le center
// TODO: Implemanter les methodes pour les types de messages qui implemantent closable (doit contenir shutdown_channel qui close un channel et shutdown_all qui close tout les channels)
// TODO: Implemanter un send pour les messages clonable et un pour les messages non clonable (faire deux blocs d'impl)

// TODO LATER: Implement drop for smart_receiver such that we can unsubscribe automatically. It means we should create a wrapper on it that contains a pointer behind a mutex to the dispatch center on which we can call unsubscribe
// TODO LATER: Add the wait for closing notifier. Does the same job as the creation waiters, but for the destruction.
// TODO LATER: Include the possibility to create WritingHandler
// TODO LATER: Include possibility to chose what you want to wait in WritingHandler
// TODO LATER: Improve error gestion in Writing Handler by adding possibility to know which sender failed
// TODO LATER: This one should be passed in argument to some function by user, but its not implemented yet.
/// The size of a notifier channel
pub(crate) const CHANNEL_SIZE: usize = 100;
/// The size of a notifier channel
pub(crate) const NOTIFIER_CHANNEL_SIZE: usize = 10;

/// This represents the state of a channel and you can have it by calling channel_state on the dispatch center
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum ChannelState {
    /// This is the basic state of the channel, there is no subscriber and in fact this channel has never had a subscriber.
    Uninitialised,
    /// The process has some subscriber, not that as long as you did not call the clean method to remove unvalid channel, the state remain running
    Running,
    /// In the over state, the channel does not have any subscriber but had once some subscriber that called the unsubscribed method.
    Over,
}

/// `SmartChannelId` is a unique identifier for channels within a `DispatchCenter`.
/// It is composed of a monotonically increasing counter and the memory address of the `DispatchCenter`
/// (converted to `usize`). This ensures that the ID is unique across any context.
///
/// The address corresponds to a specific field of a specific `DispatchCenter`, making it globally unique.
/// We store the address as a `usize` instead of a raw pointer to avoid lifetime-related complications
/// and to keep this type simple without involving generics.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) struct SmartChannelId {
    /// A counter that increments for each created channel to ensure uniqueness.
    channel_counter: usize,
    /// The memory address of the `DispatchCenter`, stored as `usize` for simplicity (its obviously used as an id and not as an address).
    dispatch_center_address: usize,
}

pub(crate) type Msg<M> = Arc<M>;
pub(crate) type MessageSender<M> = Sender<Msg<M>, SmartChannelId>;
pub type MessageReceiver<M> = Receiver<Msg<M>, SmartChannelId>;

pub type Waiter = Receiver<(), SmartChannelId>;
pub type WaiterSender = Sender<(), SmartChannelId>;

pub type ChannelId = &'static str;

/// This is the main data structure of the crate, it contains all the senders to subscriber and the waiters for channel creation. Id is not yet generic, but later is will be.
pub struct DispatchCenter<M: SendableMessage> {
    connection_id: usize,
    senders: HashMap<ChannelId, Vec<MessageSender<M>>>,
    waiter_senders: HashMap<ChannelId, Vec<WaiterSender>>,
}

impl<M: SendableMessage> Default for DispatchCenter<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: SendableMessage> DispatchCenter<M> {
    /// This function returns an empty center
    pub fn new() -> Self {
        DispatchCenter {
            connection_id: 0,
            senders: HashMap::new(),
            waiter_senders: HashMap::new(),
        }
    }

    /// This function is a local function that build a channel id, and advance the connection_id to assure channel unicity
    fn get_new_id(&mut self) -> SmartChannelId {
        let channel_counter = self.connection_id;
        self.connection_id += 1;
        SmartChannelId {
            dispatch_center_address: (self as *const DispatchCenter<M>) as usize,
            channel_counter,
        }
    }

    /// This function call the clean_channel method for all the initialized channels. Returns an hashmap binding each channel with its new state
    pub fn clean_all(&mut self) -> HashMap<ChannelId, ChannelState> {
        let mut map = HashMap::with_capacity(self.senders.len());
        for id in self.senders.keys().map(|k| *k).collect::<Vec<_>>() {
            map.insert(id, self.clean_channel(id));
        }
        map
    }

    /// This function first checks if there is some waiter for the given channel and if yes broadcast a notification to the waiters. This function should be called only after the creation of a sender. As notifications are unit, we can call new_cloning_broadcast for the writing handler
    fn notify_creation(&mut self, id: ChannelId) -> WritingHandler<()> {
        if let Some(waiters) = self.waiter_senders.get(id) {
            WritingHandler::new_cloning_broadcast((), waiters)
        } else {
            WritingHandler::empty()
        }
    }

    /// This functions returns a received subscribed to all the channels given in the ids array in parameter by calling multiple time the insert sender method with the same sender.
    pub fn subscribe_multiple(&mut self, ids: &[ChannelId]) -> MessageReceiver<M> {
        let (sender, receiver) = channel(CHANNEL_SIZE, self.get_new_id());
        for id in ids {
            self.insert_sender(sender.clone(), id);
        }
        receiver
    }

    /// This functions returns a received subscribed to th the channels given in parameter. If the channel is uninitialised, it insert the sender with the insert sender function
    pub fn subscribe(&mut self, id: ChannelId) -> MessageReceiver<M> {
        let (sender, receiver) = channel(CHANNEL_SIZE, self.get_new_id());
        self.insert_sender(sender, id);
        receiver
    }

    /// Returns true if the given receiver is subscribed to the given channel
    pub fn is_subscribed(&self, channel: ChannelId, receiver: &MessageReceiver<M>) -> bool {
        match self.channel_state(channel) {
            ChannelState::Running => get_senders!(self, channel)
                .iter()
                .any(|s| s.is_bound_to(receiver)),
            _ => false,
        }
    }

    /// This functions takes in parameter a receiver and returns all the channels in which the receiver is subscribed
    pub fn subscribed_list(&self, receiver: &MessageReceiver<M>) -> Vec<ChannelId> {
        self.senders
            .keys()
            .filter(|id| self.is_subscribed(id, receiver))
            .map(|id| *id)
            .collect()
    }

    /// This function takes in parameter a receiver, and remove the associated sender in the given channel, it it exists, otherwise it returns an error. Returns the new state of the channel.
    pub fn unsubscribe(
        &mut self,
        id: ChannelId,
        receiver: &MessageReceiver<M>,
    ) -> Result<ChannelState, DispatchError<M>> {
        match self.channel_state(id) {
            ChannelState::Running => {
                if !self.is_subscribed(id, receiver) {
                    return Err(DispatchError::NotSubscribed(id));
                }
                match self.senders.get_mut(id) {
                    Some(senders) => {
                        *senders = senders
                            .drain(..)
                            .filter(|sender| !sender.is_bound_to(receiver))
                            .collect();
                        Ok(self.channel_state(id))
                    }
                    None => Err(DispatchError::UnexpectedError(
                        UnexpectedErrorKind::InvalidChannelStateUnsubscribe,
                    )), // Should never append as we already checked the state
                }
            }
            ChannelState::Over => Err(DispatchError::ChannelOver(id)),
            ChannelState::Uninitialised => Err(DispatchError::ChannelUninitialized(id)),
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
    ) -> Result<(), DispatchError<M>> {
        let mut errors = Vec::new();
        for id in ids {
            if let Err(e) = self.unsubscribe(id, receiver) {
                errors.push(e)
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(DispatchError::NotSubscribeMultiple(errors))
        }
    }

    /// This function insert the sender in the sender and call notify creation to notify the waiter of the channel creation
    /// It writing handler of the notify creation is ignored for now as i don't really now if it is a good idea to returns
    /// it as it would imply to returns a tupple instead of just the single receiver for the subscribe methods.
    fn insert_sender(&mut self, sender: MessageSender<M>, id: ChannelId) {
        match self.senders.get_mut(id) {
            Some(senders) => senders.push(sender),
            None => {
                self.senders.insert(id, vec![sender]);
            }
        }
        // Maybe we should wait it here ?
        let _ = self.notify_creation(id);
    }

    /// This function returns a waiter for the channel. The waiter is notified each time someone subscribe to the channel
    pub fn get_waiter(&mut self, id: ChannelId) -> Waiter {
        let (sender, receiver) = channel(NOTIFIER_CHANNEL_SIZE, self.get_new_id());
        match self.waiter_senders.get_mut(id) {
            Some(w) => w.push(sender),
            None => {
                self.waiter_senders.insert(id, vec![sender]);
            }
        }
        receiver
    }

    /// This function returns the number of waiter for a given channel
    pub fn number_of_waiter(&self, id: ChannelId) -> usize {
        match self.waiter_senders.get(id) {
            Some(w) => w.len(),
            None => 0,
        }
    }

    /// This function returns the state of the given channel.
    pub fn channel_state(&self, id: ChannelId) -> ChannelState {
        match self.senders.get(id) {
            Some(s) if !s.is_empty() => ChannelState::Running,
            Some(_) => ChannelState::Over,
            None => ChannelState::Uninitialised,
        }
    }

    /// Returns the current number of subscriber for a given channel. Returns 0 if the channel is uninitialised or is over
    pub fn channel_number_subscriber(&self, id: ChannelId) -> usize {
        match self.channel_state(id) {
            ChannelState::Over | ChannelState::Uninitialised => 0,
            ChannelState::Running => get_senders!(self, id).len(),
        }
    }

    /// Create a writing handler that takes all the senders for the channel. Returns an error if the channel is not initialised and an empty writer if the channel is over
    pub async fn send(&self, msg: M) -> Result<WritingHandler<Arc<M>>, ()> {
        let id = msg.to_id();
        match self.channel_state(id) {
            ChannelState::Running => Ok(WritingHandler::new_arc_broadcast(
                msg,
                get_senders!(self, id),
            )),
            ChannelState::Over => Ok(WritingHandler::empty()),
            ChannelState::Uninitialised => todo!(),
        }
    }

    /// Clean the closed connection by testing with each sender if its close or not. Returns the state of the channel after the clean
    pub fn clean_channel(&mut self, channel: ChannelId) -> ChannelState {
        let state = self.channel_state(channel);
        match state {
            ChannelState::Running => {
                let senders = match self.senders.get_mut(channel) {
                    Some(s) => s,
                    None => return state, // Should never append
                };
                *senders = senders.drain(..).filter(|s| !s.is_closed()).collect();
                if senders.is_empty() {
                    ChannelState::Over
                } else {
                    ChannelState::Running
                }
            }
            _ => state,
        }
    }

    /// This function takes a msg anf send to all the channels. It basically just takes all the senders and build a huge writing handler with all the senders. This function calls clean_all before processing to prevent unvalid receiver to jeopardise the operation
    pub async fn broadcast(&mut self, msg: M) -> WritingHandler<Arc<M>> {
        self.clean_all();
        let senders: Vec<_> = self
            .senders
            .values()
            .flat_map(|s| s.iter().cloned())
            .collect();
        WritingHandler::new_arc_broadcast(msg, &senders)
    }

    /// This function takes in parameter a receiver, a channel and returns if it exists the senders for the given receiver in the given channel. Returns None if it doesn't exist
    pub fn get_sender(
        &self,
        receiver: &MessageReceiver<M>,
        channel: ChannelId,
    ) -> Option<MessageSender<M>> {
        todo!()
    }

    /// Does the same job as get_sender but returns multiple sender as it taks multiple channel. The function returns a hashmap binding id with corresponding sender
    pub fn get_senders(
        &self,
        receiver: &MessageReceiver<M>,
        channel: &[ChannelId],
    ) -> HashMap<ChannelId, MessageSender<M>> {
        todo!()
    }
}
