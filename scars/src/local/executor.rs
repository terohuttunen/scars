use super::LocalStorage;
use crate::Priority;
use crate::task::executor::BlockOnError;
use crate::task::{ExecutorHandle, JoinHandle, RawTask, Task, TaskHandle};
use crate::time::Instant;
use core::future::Future;
use core::pin::Pin;
use core::task::Poll;

pub struct LocalExecutor;

impl LocalExecutor {
    pub fn is_available() -> bool {
        LocalStorage::contains::<ExecutorHandle>()
    }

    pub fn get() -> &'static ExecutorHandle {
        let p = LocalStorage::as_ptr::<ExecutorHandle>().unwrap();
        // SAFETY: `ExecutorHandle` is a small `Copy` value that the
        // codebase only ever reads — there are no `with_mut::<ExecutorHandle>`
        // / `set::<ExecutorHandle>` paths. Returning `&'static` here is the
        // long-standing contract of `LocalExecutor::get()`.
        unsafe { &*p }
    }

    pub fn task_sleep_until(task: Pin<&mut RawTask>, deadline: Instant) {
        LocalStorage::with::<ExecutorHandle, _>(|executor| {
            let raw = unsafe { &*executor.raw() };
            raw.task_sleep_until(task, deadline);
        })
        .unwrap();
    }

    pub fn spawn<T>(task_handle: TaskHandle<T>) -> JoinHandle<T> {
        LocalStorage::with::<ExecutorHandle, _>(|executor| {
            let raw = unsafe { &*executor.raw() };
            raw.spawn(task_handle.as_raw());
        })
        .unwrap();
        JoinHandle::new(task_handle)
    }

    pub fn priority() -> Priority {
        LocalStorage::with::<ExecutorHandle, _>(|e| e.priority()).unwrap()
    }

    pub fn resume_task(&'static self, task: Pin<&RawTask>) {
        LocalStorage::with::<ExecutorHandle, _>(|executor| {
            let raw = unsafe { &*executor.raw() };
            raw.resume_task(task);
        })
        .unwrap();
    }

    pub fn resume_pending_tasks(&self) {
        LocalStorage::with::<ExecutorHandle, _>(|executor| {
            let raw = unsafe { &*executor.raw() };
            raw.resume_pending_tasks();
        })
        .unwrap();
    }

    pub fn block_on<F: Future>(future: F) -> F::Output {
        // The handle is needed across `init`, `block_on`, and the output
        // read below — too interleaved for a single closure. Use the
        // raw-pointer escape; same justification as `LocalExecutor::get()`.
        let executor: &'static ExecutorHandle =
            unsafe { &*LocalStorage::as_ptr::<ExecutorHandle>().unwrap() };
        let pinned_task = core::pin::pin!(Task::new());
        let task_handle = pinned_task.init(future);

        if let Err(BlockOnError::NotSupported) = executor.block_on(task_handle.as_raw()) {
            panic!("block_on() is not supported in this context")
        }

        let mut output = Poll::Pending;
        task_handle.try_read_output(&mut output, core::task::Waker::noop());
        match output {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("Task was not ready after block_on"),
        }
    }
}
