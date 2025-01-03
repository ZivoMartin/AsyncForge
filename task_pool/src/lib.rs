//! # Your Crate
//! A simple and flexible task pool for asynchronous task execution in Rust.
//! This library provides a `TaskPool` to manage concurrent tasks efficiently.

/// This module contains error handling types.
pub mod errors;
/// Contains the trait definitions for tasks.
pub mod task;
/// The implementation of the task pool.
pub mod task_pool;

pub use task::TaskTrait;
pub use task_pool::{OpId, TaskPool};
use tokio::sync::mpsc::channel;
pub use tokio::sync::mpsc::{Receiver, Sender};

pub type Channel<T> = (Sender<T>, Receiver<T>);
pub fn new_channel<T: Send>() -> Channel<T> {
    tokio::sync::mpsc::channel(100) // Taille configurable
}
