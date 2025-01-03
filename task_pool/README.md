# Task Pool

A flexible and asynchronous task pool for concurrent task execution in Rust. `Task Pool` allows you to spawn tasks, send messages, and handle task outputs efficiently using Tokio channels.

---

## **Table of Contents**
- [Features](#features)
- [Installation](#installation)
- [Usage](#usage)
  - [Basic Example](#basic-example)
  - [Waiting for Task Creation](#waiting-for-task-creation)
  - [Handling Concurrent Tasks](#handling-concurrent-tasks)
- [Error Handling](#error-handling)
- [Documentation](#documentation)
- [License](#license)

---

## **Features**
- Spawn and manage multiple asynchronous tasks.
- Send messages to tasks and receive their outputs.
- Track task states (`Running`, `Closed`).
- Automatically notify when tasks complete or when the pool is cleaned.
- Flexible task creation with support for generic task arguments.
- Customizable error handling using different error handlers.

---

## **Installation**
To add `task_pool` to your project, include the following in your `Cargo.toml`:
```toml
[dependencies]
task_pool = "0.1.0"  # Replace with the latest version
tokio = { version = "1", features = ["full"] }
```

---

## **Features**

# Basic Example

Below is a simple example using the `TaskPool` to create and run a basic task:
use task_pool::{TaskPool, TaskTrait, channel};

```rust
struct EchoTask;

impl TaskTrait<String, String, String> for EchoTask {
    fn begin(_: String, task_interface: TaskInterface<String>) -> Sender<String> {
        let (sender, mut receiver) = channel(1);
        tokio::spawn(async move {
            while let Some(input) = receiver.recv().await {
                task_interface
                    .output(format!("Echo: {input}"))
                    .await
                    .unwrap();
            }
        });
        sender
    }
}

#[tokio::main]
async fn main() {
    let (task_pool, _) = TaskPool::<String, String>::new();

    let task_id = 1;
    task_pool.new_task::<EchoTask, _>(task_id, "Hello".to_string()).await.unwrap();
    task_pool.send(task_id, "Hello again!".to_string()).await.unwrap();

    let mut result_receiver = task_pool.new_result_redirection().await;
    let result = result_receiver.recv().await.unwrap();
    assert_eq!(result.output.as_ref(), "Echo: Hello again!");
}
```

# Waiting for Task Creation

You can ensure that a task is fully created before sending a message using wait_for_task_creation:
```rust
let task_id = 42;
tokio::spawn(async move {
    task_pool.wait_for_task_creation(task_id).await.unwrap();
    task_pool.send(task_id, "Message after creation".to_string()).await.unwrap();
});
```

# Handling Concurrent Tasks

You can spawn and manage multiple tasks concurrently:
```rust
struct IncrementTask;

impl TaskTrait<u64, u64, u64> for IncrementTask {
    fn begin(init_val: u64, task_interface: TaskInterface<u64>) -> Sender<u64> {
        let (sender, mut receiver) = channel(1);
        tokio::spawn(async move {
            while let Some(val) = receiver.recv().await {
                task_interface.output(init_val + val).await.unwrap();
            }
        });
        sender
    }
}

for i in 0..5 {
    task_pool.new_task::<IncrementTask, _>(i, i).await.unwrap();
    task_pool.send(i, 10).await.unwrap();
}
```

---

## **Error Handling**

`task_pool` provides several built-in error handling mechanisms:
    - `panic_error_handler`: Panics when an error occurs
    - `log_error_handler`: Logs the errors
    - `ignore_error_handler`: Silently ignores all errors
To use a custom error handler, pass the appropriate receiver when creating the pool:
```rust
use task_pool::log_error_handler;

let (task_pool, error_receiver) = TaskPool::new();
log_error_handler(error_receiver);
```
You can also create your own error handler by using tje error receiver.

## **Documentation**

For more details on API usage, visit the docs.rs page

## **Licence**

This project is licensed under the MIT License.
Let me know if you’d like any changes or additions!
