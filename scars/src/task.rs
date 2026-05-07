pub mod executor;
pub mod raw_task;
pub mod task_pool;
use crate::events::Events;
use crate::time::{Duration, Instant};
use crate::tls::LocalStorage;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};
pub use executor::{
    EventHandlerExecutor, EventHandlerExecutorBuilder, EventHandlerExecutorHandle, ExecutorHandle,
    LocalExecutor, ThreadExecutor,
};
pub use raw_task::{RawTask, Task, TaskHandle, TaskReadyListTag};
pub use task_pool::TaskPool;

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

pub struct Sleep {
    deadline: Instant,
    // To make sure that Timer is not Send or Sync
    _phantom: PhantomData<*const ()>,
}

impl Sleep {
    pub fn sleep(duration: Duration) -> Sleep {
        Sleep {
            deadline: Instant::now() + duration,
            _phantom: PhantomData,
        }
    }

    pub fn sleep_until(deadline: Instant) -> Sleep {
        Sleep {
            deadline,
            _phantom: PhantomData,
        }
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn is_elapsed(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        let this = self.get_mut();
        let now = Instant::now();
        if now >= this.deadline {
            Poll::Ready(())
        } else {
            let task = unsafe { &mut *(cx.waker().data() as *mut RawTask) };
            // Safe: a polled task has always been spawned, so the executor is set.
            let raw_executor = task
                .get_executor()
                .expect("polled task has no executor")
                .raw();
            let pinned_task = unsafe { Pin::new_unchecked(task) };
            let executor = unsafe { &*raw_executor };
            executor.task_sleep_until(pinned_task, this.deadline);
            Poll::Pending
        }
    }
}

/// Future for waiting on events from the executor's event handler.
///
/// This is used by async tasks to wait for events sent via
/// `EventHandlerExecutorHandle::send_events()` or from interrupt handlers.
///
/// # Example
///
/// ```ignore
/// const BUTTON_EVENT: Events = 1 << 0;
///
/// // In async task:
/// loop {
///     let events = WaitForEvents::new(BUTTON_EVENT).await;
///     // Handle button press
/// }
///
/// // In interrupt handler:
/// executor_handle.send_events(BUTTON_EVENT);
/// ```
pub struct WaitForEvents {
    /// Events to wait for (mask)
    events: Events,
    /// Whether to wait for any (true) or all (false) of the events
    wait_any: bool,
    // To make sure that WaitForEvents is not Send or Sync
    _phantom: PhantomData<*const ()>,
}

impl WaitForEvents {
    /// Create a new WaitForEvents that waits for ANY of the specified events
    pub fn new(events: Events) -> WaitForEvents {
        WaitForEvents {
            events,
            wait_any: true,
            _phantom: PhantomData,
        }
    }

    /// Create a new WaitForEvents that waits for ALL of the specified events
    pub fn all(events: Events) -> WaitForEvents {
        WaitForEvents {
            events,
            wait_any: false,
            _phantom: PhantomData,
        }
    }

    /// Wait for any of the specified events
    pub fn any(events: Events) -> WaitForEvents {
        Self::new(events)
    }
}

impl Future for WaitForEvents {
    type Output = Events;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Events> {
        let task = unsafe { &*(cx.waker().data() as *const RawTask) };
        let executor = task.get_executor().expect("polled task has no executor");

        if !executor.supports_events() {
            panic!("WaitForEvents requires an event-handler executor");
        }

        // wait_any: clear matching bits in one atomic op.
        // wait_all: only clear if all required bits are present (peek
        // first; the peek/consume race is the same one the previous
        // implementation had).
        let consumed = if self.wait_any {
            executor.consume_events(self.events)
        } else {
            let pending = executor.peek_events();
            if (pending & self.events) == self.events {
                executor.consume_events(self.events)
            } else {
                0
            }
        };

        if consumed != 0 {
            Poll::Ready(consumed)
        } else {
            // Push back into the executor's atomic pending-ready queue so
            // the next event arrival (which queues `executor_poll_handler`)
            // re-polls us.
            let pinned = unsafe { Pin::new_unchecked(task) };
            executor.resume_task(pinned);
            Poll::Pending
        }
    }
}
