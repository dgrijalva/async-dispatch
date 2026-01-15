#![doc = include_str!("../README.md")]
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::{
    ffi::c_void,
    future::Future,
    pin::Pin,
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

mod sys {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/dispatch_sys.rs"));
}

/// Error returned when awaiting a task that cannot produce a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinError {
    /// The task was aborted before completion.
    Aborted,
    /// The task was polled after already returning a result.
    PollAfterReady,
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinError::Aborted => write!(f, "task was aborted"),
            JoinError::PollAfterReady => write!(f, "task polled after completion"),
        }
    }
}

impl std::error::Error for JoinError {}

enum TaskState<T> {
    Running(async_task::Task<T>),
    Completed,
    Aborted,
}

/// A handle to a spawned task that can be awaited for its result.
///
/// Dropping a Task without awaiting it allows the task to continue running
/// in the background; the result is simply discarded. This matches tokio's
/// `JoinHandle` semantics.
pub struct Task<T>(TaskState<T>);

impl<T> Task<T> {
    /// Abort the task, cancelling it at the next yield point.
    ///
    /// After calling abort, awaiting this task will return `Err(JoinError::Aborted)`.
    pub fn abort(&mut self) {
        self.0 = TaskState::Aborted;
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if let TaskState::Running(task) = std::mem::replace(&mut self.0, TaskState::Completed) {
            task.detach();
        }
    }
}

impl<T> Future for Task<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.0 {
            TaskState::Running(task) => match Pin::new(task).poll(cx) {
                Poll::Ready(val) => {
                    self.0 = TaskState::Completed;
                    Poll::Ready(Ok(val))
                }
                Poll::Pending => Poll::Pending,
            },
            TaskState::Completed => Poll::Ready(Err(JoinError::PollAfterReady)),
            TaskState::Aborted => Poll::Ready(Err(JoinError::Aborted)),
        }
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
    Task(TaskState::Running(task))
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
    Task(TaskState::Running(task))
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
                let when =
                    sys::dispatch_time(sys::DISPATCH_TIME_NOW as u64, duration.as_nanos() as i64);
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
    Task(TaskState::Running(task))
}

/// Returns a future that completes after the specified duration.
///
/// This is the async equivalent of `std::thread::sleep`. The timer is
/// managed by GCD and does not block any threads while waiting.
///
/// Note: The timer cannot be cancelled. If the `Sleep` future is dropped
/// before completion, the underlying GCD timer still fires but does nothing.
pub fn sleep(duration: Duration) -> Sleep {
    Sleep {
        duration,
        state: None,
    }
}

/// A future that completes after a duration.
///
/// Created by the [`sleep`] function.
pub struct Sleep {
    duration: Duration,
    state: Option<Arc<SleepState>>,
}

struct SleepState {
    waker: Mutex<Option<Waker>>,
    completed: AtomicBool,
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // If we haven't started the timer yet, do so now
        if self.state.is_none() {
            let state = Arc::new(SleepState {
                waker: Mutex::new(Some(cx.waker().clone())),
                completed: AtomicBool::new(false),
            });

            // Clone for GCD to own
            let gcd_state = Arc::clone(&state);
            let ptr = Arc::into_raw(gcd_state) as *mut c_void;

            // SAFETY: We call GCD's dispatch_after_f with:
            // - A valid dispatch_time computed from DISPATCH_TIME_NOW
            // - A valid global queue handle from dispatch_get_global_queue
            // - A pointer from Arc::into_raw() which transfers one ref count to GCD
            // - sleep_trampoline, which will call Arc::from_raw() exactly once
            unsafe {
                let when = sys::dispatch_time(
                    sys::DISPATCH_TIME_NOW as u64,
                    self.duration.as_nanos() as i64,
                );
                sys::dispatch_after_f(
                    when,
                    sys::dispatch_get_global_queue(
                        sys::DISPATCH_QUEUE_PRIORITY_DEFAULT as isize,
                        0,
                    ),
                    ptr,
                    Some(sleep_trampoline),
                );
            }

            self.state = Some(state);
            return Poll::Pending;
        }

        // Timer already started - check if it's completed
        let state = self.state.as_ref().unwrap();
        if state.completed.load(Ordering::SeqCst) {
            Poll::Ready(())
        } else {
            // Update the waker in case it changed
            *state.waker.lock().unwrap() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

extern "C" fn sleep_trampoline(context: *mut c_void) {
    // SAFETY: This pointer was created by Arc::into_raw() in Sleep::poll.
    // GCD calls this exactly once, so we reclaim the Arc reference here.
    let state = unsafe { Arc::from_raw(context as *const SleepState) };
    state.completed.store(true, Ordering::SeqCst);
    let waker = state.waker.lock().unwrap().take();
    drop(state);
    if let Some(waker) = waker {
        waker.wake();
    }
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
    fn test_spawn_await() {
        let task = spawn(async { 42 });
        let result = pollster::block_on(task);
        assert_eq!(result, Ok(42));
    }

    #[test]
    fn test_spawn_drop_continues() {
        // Dropping a task lets it continue running (detach-on-drop)
        let (tx, rx) = mpsc::channel();

        drop(spawn(async move {
            tx.send(42).unwrap();
        }));

        let result = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_spawn_after_delays() {
        let start = std::time::Instant::now();

        let task = spawn_after(Duration::from_millis(100), async { 123 });
        let result = pollster::block_on(task);

        assert_eq!(result, Ok(123));
        assert!(
            start.elapsed() >= Duration::from_millis(100),
            "expected at least 100ms delay, got {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn test_sleep() {
        let start = std::time::Instant::now();

        let task = spawn(async {
            sleep(Duration::from_millis(100)).await;
            "done"
        });
        let result = pollster::block_on(task);

        assert_eq!(result, Ok("done"));
        assert!(
            start.elapsed() >= Duration::from_millis(100),
            "expected at least 100ms delay, got {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn test_sleep_zero_duration() {
        let task = spawn(async {
            sleep(Duration::ZERO).await;
            "done"
        });
        let result = pollster::block_on(task);
        assert_eq!(result, Ok("done"));
    }

    #[test]
    fn test_abort() {
        let (tx, rx) = mpsc::channel();

        let mut task = spawn(async move {
            sleep(Duration::from_millis(200)).await;
            tx.send(()).unwrap();
            42
        });

        // Give it time to start
        std::thread::sleep(Duration::from_millis(10));

        task.abort();

        // The channel should never receive (task was cancelled)
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());

        // Awaiting returns Aborted
        let result = pollster::block_on(task);
        assert_eq!(result, Err(JoinError::Aborted));
    }

    #[test]
    fn test_nested_spawn_await() {
        let task = spawn(async {
            let inner = spawn(async { 42 });
            inner.await.unwrap() + 1
        });

        let result = pollster::block_on(task);
        assert_eq!(result, Ok(43));
    }

    #[test]
    fn test_poll_after_ready() {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll, Waker};

        let mut task = spawn(async { 1 });

        // Poll to completion
        let waker = Waker::noop();
        let mut cx = Context::from_waker(&waker);
        loop {
            match Pin::new(&mut task).poll(&mut cx) {
                Poll::Ready(Ok(1)) => break,
                Poll::Ready(other) => panic!("unexpected result: {:?}", other),
                Poll::Pending => std::thread::yield_now(),
            }
        }

        // Poll again after ready
        let result = Pin::new(&mut task).poll(&mut cx);
        assert!(matches!(
            result,
            Poll::Ready(Err(JoinError::PollAfterReady))
        ));
    }
}
