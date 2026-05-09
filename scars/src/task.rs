pub mod event_handler_executor;
pub mod executor;
pub mod raw_task;
pub mod sleep;
pub mod task_pool;
pub mod thread_executor;
pub mod wait_for_events;

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
pub use thread_executor::ThreadExecutor;
pub use wait_for_events::WaitForEvents;

pub struct JoinHandle<T> {
    task_handle: Option<TaskHandle<T>>,
}

impl<T> JoinHandle<T> {
    pub fn new(task_handle: TaskHandle<T>) -> JoinHandle<T> {
        JoinHandle {
            task_handle: Some(task_handle),
        }
    }

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
