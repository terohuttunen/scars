use super::JoinHandle;
use super::executor::{BlockOnError, Executor, ExecutorHandle, RawExecutor};
use super::raw_task::{RawTask, RawTaskHandle, TaskHandle};
use crate::Priority;
use crate::events::{EXECUTOR_WAKEUP_EVENT, EventOptions, WaitEvents};
use crate::local::{LocalCell, Publish, PublishCtx, PublishError};
use crate::thread::ThreadRef;
use crate::time::Instant;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::Poll;

pub struct RawThreadExecutor {
    raw: RawExecutor,
    thread: ThreadRef,
    /// Cell used by [`Publish`] to install this executor's handle into
    /// a thread's local-storage namespace.
    handle_cell: LocalCell<ExecutorHandle>,
}

#[allow(dead_code)]
impl RawThreadExecutor {
    pub const fn new(thread: ThreadRef) -> Self {
        Self {
            raw: RawExecutor::new(),
            thread,
            handle_cell: LocalCell::new(),
        }
    }

    fn spawn(&'static self, task_handle: &RawTaskHandle) {
        unsafe { RawTask::set_executor(task_handle.raw_task_ptr(), self.handle()) };
        // The atomic pending-ready queue; `resume_task` also notifies the
        // thread, so a spawn from another context wakes a running executor.
        self.resume_task(task_handle.as_ref());
    }

    fn block_on(&'static self, task_handle: &RawTaskHandle) {
        unsafe { RawTask::set_executor(task_handle.raw_task_ptr(), self.handle()) };
        // No notify needed: the loop below polls immediately, and `poll`
        // drains the pending-ready queue first.
        self.raw.resume_task(task_handle.as_ref());

        loop {
            let poll_result = self.raw.poll();

            if task_handle.poll() {
                break;
            }

            let mut context =
                WaitEvents::with_options(EXECUTOR_WAKEUP_EVENT, EventOptions::wait_any());

            if let Some(deadline) = poll_result.deadline_opt {
                let _ = context.wait_until(deadline);
            } else {
                let _ = context.wait();
            }
        }
    }

    // Safe to call from ISR or another thread
    fn resume_task(&'static self, task: Pin<&RawTask>) {
        self.raw.resume_task(task);
        self.thread.send_events(EXECUTOR_WAKEUP_EVENT);
    }

    fn task_sleep_until(&'static self, task: Pin<&mut RawTask>, deadline: Instant) {
        self.raw.task_sleep_until(task, deadline);
    }

    fn resume_pending_tasks(&'static self, notify_executor: bool) {
        let task_became_ready = self.raw.resume_pending_tasks();
        if task_became_ready & notify_executor {
            self.thread.send_events(EXECUTOR_WAKEUP_EVENT);
        }
    }

    pub fn priority(&self) -> Priority {
        self.thread.base_priority()
    }

    pub fn as_raw(&'static self) -> &'static RawExecutor {
        &self.raw
    }
}

impl Executor for RawThreadExecutor {
    const SUPPORTS_BLOCK_ON: bool = true;

    fn raw(&'static self) -> *const RawExecutor {
        &self.raw
    }
    fn notify(&'static self) {
        self.thread.send_events(EXECUTOR_WAKEUP_EVENT);
    }
    fn priority(&self) -> Priority {
        self.thread.base_priority()
    }
    fn block_on(&'static self, task: &RawTaskHandle) -> Result<(), BlockOnError> {
        Self::block_on(self, task);
        Ok(())
    }
    fn resume_task(&'static self, task: Pin<&RawTask>) {
        Self::resume_task(self, task);
    }
    // send_events / peek_events / consume_events inherit defaults.
}

impl Publish for ThreadExecutor {
    fn try_publish_to(&'static self, ctx: &mut PublishCtx<'_>) -> Result<(), PublishError> {
        ctx.put_init(&self.raw.handle_cell, |_| self.raw.handle())?;
        Ok(())
    }
}

pub struct ThreadExecutor {
    raw: RawThreadExecutor,
    // To make sure that ThreadExecutor is not Send or Sync
    _phantom: PhantomData<*const ()>,
}

#[allow(dead_code)]
impl ThreadExecutor {
    pub fn new() -> ThreadExecutor {
        let thread = unsafe { ThreadRef::current() };
        ThreadExecutor {
            raw: RawThreadExecutor::new(thread),
            _phantom: PhantomData,
        }
    }

    pub fn spawn<T>(&'static self, task_handle: TaskHandle<T>) -> JoinHandle<T> {
        self.raw.spawn(task_handle.as_raw());
        JoinHandle::new(task_handle)
    }

    pub fn block_on<T>(&'static self, task_handle: TaskHandle<T>) -> T {
        let mut output = Poll::Pending;
        self.raw.block_on(task_handle.as_raw());
        task_handle.try_read_output(&mut output, core::task::Waker::noop());
        match output {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("Task was not ready after block_on"),
        }
    }

    fn priority(&self) -> Priority {
        self.raw.thread.base_priority()
    }

    pub(crate) fn resume_task(&'static self, task: Pin<&RawTask>) {
        self.raw.resume_task(task);
    }

    pub(crate) fn task_sleep_until(&'static self, task: Pin<&mut RawTask>, deadline: Instant) {
        self.raw.task_sleep_until(task, deadline);
    }

    fn resume_pending_tasks(&'static self) {
        self.raw.resume_pending_tasks(true)
    }

    #[allow(dead_code)]
    fn as_raw(&'static self) -> &'static RawThreadExecutor {
        &self.raw
    }

    pub fn handle(&'static self) -> ExecutorHandle {
        self.raw.handle()
    }
}
