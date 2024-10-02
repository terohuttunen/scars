use super::{InterruptExecutor, JoinHandle, LocalExecutor, Task, ThreadExecutor};
use crate::tls::LocalStorage;
use crate::{ExecutionContext, Scheduler};
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

    pub fn spawn(&'static self, future: F) -> Result<JoinHandle<F::Output>, ()> {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(_) => {
                let interrupt_executor = LocalStorage::get::<InterruptExecutor>().unwrap();
                self.spawn_in_interrupt(interrupt_executor, future)
            }
            ExecutionContext::Thread(_) => {
                let thread_executor = LocalStorage::get::<ThreadExecutor>().unwrap();
                self.spawn_in_thread(thread_executor, future)
            }
        }
    }

    pub fn spawn_in_thread(
        &'static self,
        executor: &'static ThreadExecutor,
        future: F,
    ) -> Result<JoinHandle<F::Output>, ()> {
        self.task_cells
            .iter()
            .find_map(|task_cell| unsafe { &mut *task_cell.get() }.claim())
            .map(|task| Pin::static_mut(task))
            .map(|pinned_task| {
                let task_handle = pinned_task.init(future, LocalExecutor::Thread(executor));
                Ok(executor.spawn(task_handle))
            })
            .unwrap_or(Err(()))
    }

    pub fn spawn_in_interrupt(
        &'static self,
        executor: &'static InterruptExecutor,
        future: F,
    ) -> Result<JoinHandle<F::Output>, ()> {
        self.task_cells
            .iter()
            .find_map(|task_cell| unsafe { &mut *task_cell.get() }.claim())
            .map(|task| Pin::static_mut(task))
            .map(|pinned_task| {
                let task_handle = pinned_task.init(future, LocalExecutor::Interrupt(executor));
                Ok(executor.spawn(task_handle))
            })
            .unwrap_or(Err(()))
    }
}

unsafe impl<F: Future, const N: usize> Sync for TaskPool<F, N> {}
