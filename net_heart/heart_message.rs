use global_lib::{process_pool::PoolProcessEnded, NodeId, OpId};
use sendable_proc_macros::Sendable;

use crate::{
    crypto::data_structures::keypair::PublicKey, node::node_process_output::NodeProcessOutput,
};

use super::{nodes_heart::SetupData, summaries::Summaries};

#[derive(Clone, Eq, PartialEq)]
pub struct NewMessage {
    pub bytes: Vec<u8>,
    pub sender: NodeId,
    pub id: OpId,
}

#[derive(Clone, Sendable)]
pub enum HeartMessage {
    MessageSender(NewMessage),
    SetupOver(SetupData),
    NetworkCleared,
    Key(u16, PublicKey),
    PoolOutput(PoolProcessEnded<NodeProcessOutput>),
    EmitSumm(Summaries),
    EmitN(usize),
    GiveSumm(usize),
    Close,
}
