use super::{LocalExecutor, Task, TaskHandle};

use core::cell::UnsafeCell;
use core::future::Future;
use core::pin::Pin;

#[macro_export]
macro_rules! make_task_pool {
    ( $size:expr ) => {{
        type F = impl ::core::future::Future;
        static TASK_POOL: $crate::task::TaskPool<F, { $size }> = $crate::task::TaskPool::new();
        &TASK_POOL
    }};
}

pub struct TaskPool<F: Future, const N: usize> {
    task_cells: [UnsafeCell<Task<F>>; N],
}

impl<F: Future, const N: usize> TaskPool<F, N> {
    const TASK_CELL_INITIALIZER: UnsafeCell<Task<F>> = UnsafeCell::new(Task::INITIALIZER);
    pub const fn new() -> TaskPool<F, N> {
        TaskPool {
            task_cells: [Self::TASK_CELL_INITIALIZER; N],
        }
    }

    pub fn alloc(&'static self) -> Option<TaskBuilder<F>> {
        self.task_cells
            .iter()
            .find_map(|task_cell| unsafe { &mut *task_cell.get() }.claim())
            .map(|task| Pin::static_mut(task))
            .map(|pinned_task| TaskBuilder { task: pinned_task })
    }
}

unsafe impl<F: Future, const N: usize> Sync for TaskPool<F, N> {}

pub struct TaskBuilder<F: Future + 'static> {
    task: Pin<&'static mut Task<F>>,
}

impl<F: Future> TaskBuilder<F> {
    pub fn attach<C: FnOnce() -> F>(self, future: C) -> InitializedTask<F::Output> {
        let future = future();
        // Task is initialized without executor
        InitializedTask {
            task_handle: self.task.init(future, LocalExecutor::None),
        }
    }
}

// Task is initialized and ready to be executed. Executor has not been yet assigned.
pub struct InitializedTask<T> {
    task_handle: TaskHandle<T>,
}

impl<T> InitializedTask<T> {
    pub(crate) fn with_executor(mut self, executor: LocalExecutor) -> TaskHandle<T> {
        unsafe {
            self.task_handle.as_mut().set_executor(executor);
        }
        self.task_handle
    }
}
