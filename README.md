# async-dispatch

Async task dispatch via Grand Central Dispatch (GCD) for Apple platforms.

Instead of managing your own async runtime (like tokio), this crate hands off task scheduling to the operating system's native dispatch queues. You spawn futures; GCD handles the rest.

## Documentation

[API docs](https://dgrijalva.github.io/async-dispatch) are hosted on GitHub Pages. Since this crate only builds on Apple platforms, docs.rs cannot generate documentation for it.

## Usage

```rust,ignore
use async_dispatch::{spawn, spawn_on_main, spawn_after, sleep, JoinError};
use std::time::Duration;

// Spawn on a background queue and await the result
spawn(async {
    let task = spawn(async {
        expensive_computation().await
    });

    // Await returns Result<T, JoinError>
    let result = task.await.unwrap();
});

// Spawn on the main thread (for UI work)
spawn_on_main(async {
    update_ui()
});

// Spawn after a delay
spawn_after(Duration::from_secs(5), async {
    delayed_work().await
});

// Sleep within an async context
spawn(async {
    do_something();
    sleep(Duration::from_secs(1)).await;
    do_something_else();
});
```

## Task lifecycle

- `task.await` - wait for completion, returns `Result<T, JoinError>`
- `task.abort()` - cancel the task; awaiting returns `Err(JoinError::Aborted)`
- dropping a task lets it run to completion (like tokio's `JoinHandle`)

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
- No async IO primitives provided (files, network, etc)

## Attribution

This approach is based on the dispatcher implementation in [Zed's gpui crate](https://github.com/zed-industries/zed/tree/main/crates/gpui), which uses GCD for async task scheduling on macOS. The gpui crate is licensed under Apache-2.0.
