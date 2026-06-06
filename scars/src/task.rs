pub mod event_handler_executor;
pub mod executor;
pub mod raw_task;
pub mod sleep;
pub mod task_pool;
#[cfg(feature = "multithreading")]
pub mod thread_executor;
pub mod wait_for_events;
#[cfg(feature = "async")]
pub mod yield_now;

#[cfg(feature = "multithreading")]
use crate::local::LocalStorage;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

pub use crate::local::LocalExecutor;
pub use event_handler_executor::{
    EventHandlerExecutor, EventHandlerExecutorBuilder, EventHandlerExecutorHandle,
};
pub use executor::ExecutorHandle;
pub use raw_task::{RawTask, Task, TaskHandle, TaskReadyListTag};
pub use sleep::Sleep;
pub use task_pool::TaskPool;
#[cfg(feature = "multithreading")]
pub use thread_executor::ThreadExecutor;
pub use wait_for_events::WaitForEvents;
#[cfg(feature = "async")]
pub use yield_now::{YieldNow, yield_now};

pub struct JoinHandle<T> {
    task_handle: Option<TaskHandle<T>>,
}

impl<T> JoinHandle<T> {
    pub fn new(task_handle: TaskHandle<T>) -> JoinHandle<T> {
        JoinHandle {
            task_handle: Some(task_handle),
        }
    }

    /// Cancel the task. Its future is dropped immediately (running its
    /// destructors, e.g. releasing a held guard) and it is never polled
    /// again. A no-op if the task has already finished. Dropping the handle
    /// instead detaches the task, leaving it running; `abort` is the explicit
    /// cancellation path.
    pub fn abort(&self) {
        if let Some(task_handle) = self.task_handle.as_ref() {
            task_handle.as_raw().abort();
        }
    }

    #[cfg(feature = "multithreading")]
    pub fn join(self) -> T {
        let task_handle = self
            .task_handle
            .expect("JoinHandle polled after completion");

        let p = LocalStorage::as_ptr::<ThreadExecutor>().unwrap();
        // SAFETY: `block_on` is read-only on the executor handle and
        // there is no `with_mut`/`set` path for `ThreadExecutor`.
        let executor: &'static ThreadExecutor = unsafe { &*p };
        executor.block_on(task_handle)
    }

    pub fn is_finished(&self) -> bool {
        self.task_handle.is_none()
    }
}

impl<T> Unpin for JoinHandle<T> {}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let mut ret = Poll::Pending;
        if let Some(task_handle) = &self.task_handle {
            task_handle.try_read_output(&mut ret, cx.waker());

            if ret.is_ready() {
                self.get_mut().task_handle = None;
            }
        } else {
            panic!("JoinHandle polled after completion");
        }
        ret
    }
}

/// Resolve the running task and a copy of its executor handle from a poll
/// [`Context`]. Sound only when the future is driven by a SCARS executor:
/// the waker's data pointer is the task's [`RawTask`] (see
/// [`RawTask::waker`]) — the same invariant [`Sleep`]/[`WaitForEvents`]
/// rely on. Used by the async sync primitives and combinators to self-requeue
/// (`resume_task`) and wake (`notify`) the executor.
pub(crate) fn task_and_executor<'t>(cx: &Context<'_>) -> (Pin<&'t RawTask>, ExecutorHandle) {
    // The waker data is the task pointer; the task outlives this poll, so the
    // borrow is sound for any caller-chosen lifetime (as in `sleep.rs`).
    let task: &'t RawTask = unsafe { &*(cx.waker().data() as *const RawTask) };
    let executor = *task
        .get_executor()
        .expect("async primitive polled by a task with no executor");
    (unsafe { Pin::new_unchecked(task) }, executor)
}

pub fn spawn<F: Future, const N: usize>(
    task_pool: &'static TaskPool<F, N>,
    future: F,
) -> Result<JoinHandle<F::Output>, ()> {
    match task_pool.alloc() {
        Some(builder) => {
            let task_handle = builder.attach(|| future);
            Ok(LocalExecutor::spawn(task_handle))
        }
        None => Err(()),
    }
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    LocalExecutor::block_on(future)
}
