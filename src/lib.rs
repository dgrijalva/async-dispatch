#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::{
    ffi::c_void,
    future::Future,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
    time::Duration,
};

mod sys {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/dispatch_sys.rs"));
}

/// A handle to a spawned task that can be awaited or detached.
///
/// If dropped without calling `detach()`, the task continues running
/// but its result is discarded.
#[must_use = "tasks are cancelled when dropped, use .detach() to run in background"]
pub struct Task<T>(async_task::Task<T>);

impl<T> Task<T> {
    /// Detach the task, allowing it to run to completion in the background.
    /// The result will be discarded.
    pub fn detach(self) {
        self.0.detach()
    }
}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

/// Spawn a future on a background GCD queue.
///
/// The future will be polled on one of the system's global concurrent queues.
pub fn spawn<F, T>(future: F) -> Task<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (runnable, task) = async_task::spawn(future, schedule_background);
    runnable.schedule();
    Task(task)
}

/// Spawn a future on the main thread's dispatch queue.
///
/// Use this for work that must run on the main thread, such as UI updates.
pub fn spawn_on_main<F, T>(future: F) -> Task<T>
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    let (runnable, task) = async_task::spawn_local(future, schedule_main);
    runnable.schedule();
    Task(task)
}

/// Spawn a future on a background queue after a delay.
///
/// The delay only applies to the initial spawn. If the future yields and is
/// woken again, subsequent polls happen immediately.
pub fn spawn_after<F, T>(duration: Duration, future: F) -> Task<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let first_schedule = Arc::new(AtomicBool::new(true));
    let (runnable, task) = async_task::spawn(future, move |runnable: async_task::Runnable<()>| {
        let ptr = runnable.into_raw().as_ptr() as *mut c_void;

        if first_schedule.swap(false, Ordering::SeqCst) {
            // SAFETY: We call GCD's dispatch_after_f with:
            // - A valid dispatch_time computed from DISPATCH_TIME_NOW
            // - A valid global queue handle from dispatch_get_global_queue
            // - A pointer from Runnable::into_raw() which transfers ownership to GCD
            // - trampoline, which will reconstruct the Runnable exactly once
            unsafe {
                let when = sys::dispatch_time(
                    sys::DISPATCH_TIME_NOW as u64,
                    duration.as_nanos() as i64,
                );
                sys::dispatch_after_f(
                    when,
                    sys::dispatch_get_global_queue(
                        sys::DISPATCH_QUEUE_PRIORITY_DEFAULT as isize,
                        0,
                    ),
                    ptr,
                    Some(trampoline),
                );
            }
        } else {
            // SAFETY: We call GCD's dispatch_async_f with:
            // - A valid global queue handle from dispatch_get_global_queue
            // - A pointer from Runnable::into_raw() which transfers ownership to GCD
            // - trampoline, which will reconstruct the Runnable exactly once
            unsafe {
                sys::dispatch_async_f(
                    sys::dispatch_get_global_queue(
                        sys::DISPATCH_QUEUE_PRIORITY_DEFAULT as isize,
                        0,
                    ),
                    ptr,
                    Some(trampoline),
                );
            }
        }
    });
    runnable.schedule();
    Task(task)
}

fn dispatch_get_main_queue() -> sys::dispatch_queue_t {
    std::ptr::addr_of!(sys::_dispatch_main_q) as *const _ as sys::dispatch_queue_t
}

fn schedule_background(runnable: async_task::Runnable<()>) {
    let ptr = runnable.into_raw().as_ptr() as *mut c_void;
    // SAFETY: We call GCD's dispatch_async_f with:
    // - A valid global queue handle from dispatch_get_global_queue
    // - A pointer from Runnable::into_raw() which transfers ownership to GCD
    // - trampoline, which will reconstruct the Runnable exactly once
    unsafe {
        sys::dispatch_async_f(
            sys::dispatch_get_global_queue(sys::DISPATCH_QUEUE_PRIORITY_DEFAULT as isize, 0),
            ptr,
            Some(trampoline),
        );
    }
}

fn schedule_main(runnable: async_task::Runnable<()>) {
    let ptr = runnable.into_raw().as_ptr() as *mut c_void;
    // SAFETY: We call GCD's dispatch_async_f with:
    // - The main queue handle (a valid static queue)
    // - A pointer from Runnable::into_raw() which transfers ownership to GCD
    // - trampoline, which will reconstruct the Runnable exactly once
    unsafe {
        sys::dispatch_async_f(dispatch_get_main_queue(), ptr, Some(trampoline));
    }
}

extern "C" fn trampoline(context: *mut c_void) {
    // SAFETY: This function is only called by GCD with a pointer that was created
    // by Runnable::into_raw() in one of the schedule functions. GCD guarantees:
    // - The pointer is passed exactly once per dispatch
    // - The pointer value is unchanged from what we provided
    // We reconstruct the Runnable, taking back ownership, and run it.
    let runnable =
        unsafe { async_task::Runnable::<()>::from_raw(NonNull::new_unchecked(context as *mut ())) };
    runnable.run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn test_spawn_runs_future() {
        let (tx, rx) = mpsc::channel();

        spawn(async move {
            tx.send(42).unwrap();
        })
        .detach();

        let result = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_spawn_returns_value() {
        let (tx, rx) = mpsc::channel();

        let task = spawn(async { 123 });

        // Spawn another task to await the first and send result
        spawn(async move {
            let value = task.await;
            tx.send(value).unwrap();
        })
        .detach();

        let result = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(result, 123);
    }

    #[test]
    fn test_spawn_after_delays() {
        let (tx, rx) = mpsc::channel();
        let start = std::time::Instant::now();

        spawn_after(Duration::from_millis(100), async move {
            tx.send(()).unwrap();
        })
        .detach();

        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(100),
            "expected at least 100ms delay, got {:?}",
            elapsed
        );
    }
}
