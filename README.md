# async-dispatch

Async task dispatch via Grand Central Dispatch (GCD) for Apple platforms.

Instead of managing your own async runtime (like tokio), this crate hands off task scheduling to the operating system's native dispatch queues. You spawn futures; GCD handles the rest.

## Usage

```rust
use async_dispatch::{spawn, spawn_on_main, spawn_after, sleep};
use std::time::Duration;

// Spawn on a background queue
let task = spawn(async {
    // runs on GCD's global concurrent queue
    expensive_computation()
});

// Spawn on the main thread (for UI work)
spawn_on_main(async {
    update_ui()
}).detach();

// Spawn after a delay
let task = spawn_after(Duration::from_secs(5), async {
    delayed_work()
});

// Await the result or detach to run in background
let result = task.await;

// Sleep within an async context
spawn(async {
    do_something();
    sleep(Duration::from_secs(1)).await;
    do_something_else();
}).detach();
```

## Task lifecycle

- `task.await` - wait for completion and get the result
- `task.detach()` - let it run to completion, discard the result
- dropping a task cancels it

## Requirements

- macOS or iOS (uses libdispatch)
- Rust 2021 edition

## How it works

The crate uses [async-task](https://docs.rs/async-task) to convert Rust futures into raw function pointers that can be passed to GCD's `dispatch_async_f`. When GCD executes the function, it polls the future. If the future yields, subsequent wakeups are dispatched back to GCD.

There is no runtime to start or stop. GCD manages thread pools and scheduling.

## Limitations

- Apple platforms only (macOS, iOS, etc.)
- No priority control yet (uses default queue priority)
- No `block_on` for synchronously waiting on futures. (use `futures::block_on`)

## Attribution

This approach is based on the dispatcher implementation in [Zed's gpui crate](https://github.com/zed-industries/zed/tree/main/crates/gpui), which uses GCD for async task scheduling on macOS. The gpui crate is licensed under Apache-2.0.
