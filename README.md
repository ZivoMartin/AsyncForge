# **AsyncForge**

**AsyncForge** is a powerful and modular toolkit for managing asynchronous tasks in **Rust**, built on top of **Tokio**. It provides a suite of crates that simplify asynchronous programming through customizable tools, procedural macros, and robust communication mechanisms.

---

## **Crate Overview**

### 1. **Task Forge** (`task_forge`)

A **flexible and performant task pool** for managing asynchronous tasks. This crate allows you to spawn multiple tasks, send messages to them, and listen for their output.

**Key Features**:
- Efficiently spawn and manage concurrent tasks.
- Send and receive custom messages to tasks using channels.
- Notifications for task creation, output, and termination.

**Example**:
```rust
use task_forge::{TaskPool, TaskTrait};
use tokio::sync::mpsc::Sender;

struct EchoTask;

impl TaskTrait<String, String, String> for EchoTask {
    fn begin(_: String, task_interface: task_forge::TaskInterface<String>) -> Sender<String> {
        let (sender, mut receiver) = task_forge::channel(10);
        tokio::spawn(async move {
            if let Some(input) = receiver.recv().await {
                let response = format!("Echo: {input}");
                task_interface.output(response).await.unwrap();
            }
            });
        sender
    }
}

#[tokio::main]
async fn main() {
    let (pool, _) = TaskPool::new();
    pool.new_task::<EchoTask, _>(1, "Hello World!".to_string()).await.unwrap();
    pool.send(1, "Ping!".to_string()).await.unwrap();

    let mut results = pool.new_result_redirection().await;
    if let Some(result) = results.recv().await {
        println!("Task Output: {}", result.output);
    }
}
```
--- 

Other crates are coming soon ! 
