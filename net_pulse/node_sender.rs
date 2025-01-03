use std::collections::HashMap;
use tokio::{sync::mpsc::channel, task::JoinHandle};

pub use tokio::sync::mpsc::{Receiver, Sender};

use super::message_interface::SendableMessage;

#[derive(Default)]
pub struct WritingFuture {
    handlers: Vec<JoinHandle<Result<(), String>>>,
}

impl WritingFuture {
    pub fn new<M: SendableMessage>(msg: M, senders: &[Sender<M>]) -> Self {
        WritingFuture {
            handlers: senders
                .iter()
                .map(|sender| {
                    let msg = msg.clone();
                    let sender = sender.clone();
                    let kind = msg.to_str();
                    tokio::spawn(async move {
                        match sender.send(msg).await {
                            Ok(_) => Ok(()),
                            Err(_) => Err(format!("Failed to send a messge of kind {kind}")),
                        }
                    })
                })
                .collect(),
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub async fn wait(self) -> Result<(), ()> {
        let mut res = Ok(());
        for handler in self.handlers {
            if handler.await.is_err() {
                res = Err(())
            }
        }
        res
    }
}

pub type ChannelId = &'static str;

#[derive(Clone)]
pub struct NodeSender<M: SendableMessage> {
    senders: Vec<Option<Vec<Sender<M>>>>,
    waiter_senders: HashMap<&'static str, Vec<Sender<()>>>,
}

impl<M: SendableMessage> Default for NodeSender<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: SendableMessage> NodeSender<M> {
    pub fn new() -> Self {
        NodeSender {
            senders: vec![None; M::NB_SENDERS],
            waiter_senders: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.senders.iter_mut().for_each(|s| *s = None);
    }

    fn notify_creation(&mut self, id: ChannelId) {
        if let Some(waiters) = self.waiter_senders.remove(id) {
            tokio::spawn(async move {
                for w in waiters {
                    w.send(()).await.unwrap()
                }
            });
        }
    }

    pub fn subscribe_multiple(&mut self, ids: &[ChannelId]) -> Receiver<M> {
        let (sender, receiver) = channel::<M>(100);
        for id in ids {
            self.push_sender(sender.clone(), id);
        }
        receiver
    }

    pub fn subscribe(&mut self, id: ChannelId) -> Receiver<M> {
        let (sender, receiver) = channel::<M>(100);
        self.push_sender(sender, id);
        receiver
    }

    fn push_sender(&mut self, sender: Sender<M>, s_id: ChannelId) {
        let id = M::str_to_id(s_id);
        if self.senders[id].is_none() {
            self.notify_creation(s_id);
            self.senders[id].insert(Vec::new())
        } else {
            self.senders[id].as_mut().unwrap()
        }
        .push(sender);
    }

    pub fn get_waiter(&mut self, s_id: &'static str) -> Option<Receiver<()>> {
        if self.senders[M::str_to_id(s_id)].is_some() {
            None
        } else {
            let (sender, receiver) = channel(1000);
            match self.waiter_senders.get_mut(s_id) {
                Some(waiters) => waiters.push(sender),
                None => {
                    self.waiter_senders.insert(s_id, vec![sender]);
                }
            }
            Some(receiver)
        }
    }

    pub fn is_setup(&self, id: ChannelId) -> bool {
        self.senders[M::str_to_id(id)].is_some()
    }

    pub fn channel_number_subscriber(&self, id: ChannelId) -> usize {
        if let Some(channel) = &self.senders[M::str_to_id(id)] {
            channel.len()
        } else {
            0
        }
    }

    pub async fn send(&self, msg: M) -> WritingFuture {
        let kind = msg.to_str().to_string();
        if let Some(senders) = &self.senders[msg.get_id()] {
            WritingFuture::new(msg, senders)
        } else {
            panic!("Channel is not ready, message: {kind}")
        }
    }

    pub async fn try_send(&self, msg: M) -> Result<WritingFuture, String> {
        let kind = msg.to_str().to_string();
        if let Some(senders) = &self.senders[msg.get_id()] {
            Ok(WritingFuture::new(msg, senders))
        } else {
            Err(format!("Channel is not ready, message: {kind}"))
        }
    }

    pub fn channel_is_over(&self, channel: ChannelId) -> bool {
        if let Some(sender) = &self.senders[M::str_to_id(channel)] {
            !sender.iter().any(|sender| !sender.is_closed())
        } else {
            false
        }
    }

    pub async fn broadcast(&mut self, msg: M) {
        for senders in self.senders.iter_mut().filter_map(|s| s.as_mut()) {
            for sender in senders {
                let _ = sender.send(msg.clone()).await;
            }
        }
    }

    pub fn get_sender(&self, channel: ChannelId) -> Sender<M> {
        let id = M::str_to_id(channel);

        if let Some(senders) = &self.senders[id] {
            senders.last().unwrap().clone()
        } else {
            panic!("Channel is not ready, message: {channel}")
        }
    }

    pub async fn close(&mut self, channel: ChannelId) {
        let id = M::str_to_id(channel);
        if let Some(senders) = &self.senders[id] {
            for s in senders {
                let _ = s.send(M::close()).await;
            }
        }
        self.senders[id] = None
    }
}
