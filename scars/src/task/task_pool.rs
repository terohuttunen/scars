use super::{Task, TaskHandle};

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
    pub fn attach<C: FnOnce() -> F>(self, future: C) -> TaskHandle<F::Output> {
        let future = future();
        self.task.init(future)
    }
}
