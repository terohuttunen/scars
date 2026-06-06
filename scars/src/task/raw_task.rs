use super::ExecutorHandle;
use crate::kernel::atomic_queue::*;
use crate::kernel::list::{LinkedListTag, Node, impl_linked};
use crate::kernel::waiter::{WaitQueueEntry, WaitQueueTag};
use crate::sync::atomic::{AtomicU32, Ordering};
use crate::time::Instant;
use core::future::Future;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

pub struct TaskReadyListTag;

impl LinkedListTag for TaskReadyListTag {}

pub const ASYNC_TASK_STATE_FREE: u32 = 0;
pub const ASYNC_TASK_STATE_UNINIT: u32 = 1;
pub const ASYNC_TASK_STATE_RUNNING: u32 = 2;
pub const ASYNC_TASK_STATE_FINISHED: u32 = 3;

pub struct RawTask {
    /// `None` between `Task::init` and `Executor::spawn`; populated by the
    /// spawning executor before the task is pushed onto a queue. After spawn
    /// the task can only be polled, woken, or slept by code reached through
    /// the executor's queue, so reads can safely treat this as `Some`.
    pub(crate) executor: Option<ExecutorHandle>,
    ready_list_link: Node<RawTask, TaskReadyListTag>,
    sleep_list_link: Node<RawTask, WaitQueueTag>,
    pending_ready_list_link: AtomicNode<RawTask, TaskReadyListTag>,
    pub(crate) waiter: WaitQueueEntry,
    pub(super) wakeup_time: Instant,
    task_ptr: *mut (),
    poll_fn: unsafe fn(*mut ()) -> bool,
    on_result_waker: Option<Waker>,
    _pinned: core::marker::PhantomPinned,
}

impl RawTask {
    const TASK_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::waker_clone,
        Self::waker_wake,
        Self::waker_wake,
        Self::waker_drop,
    );

    pub fn poll(&self) -> bool {
        unsafe { (self.poll_fn)(self.task_ptr) }
    }

    pub fn waker(&self) -> Waker {
        unsafe {
            Waker::from_raw(RawWaker::new(
                self as *const _ as *const (),
                &Self::TASK_WAKER_VTABLE,
            ))
        }
    }

    fn waker_clone(data: *const ()) -> RawWaker {
        RawWaker::new(data, &Self::TASK_WAKER_VTABLE)
    }

    fn waker_wake(data: *const ()) {
        let raw = unsafe { &*(data as *const RawTask) };
        raw.executor
            .as_ref()
            .expect("waker fired before task was spawned")
            .notify();
    }

    fn waker_drop(_data: *const ()) {}

    pub fn set_wakeup_time(self: Pin<&mut Self>, wakeup_time: Instant) {
        let task = unsafe { Pin::get_unchecked_mut(self) };
        task.wakeup_time = wakeup_time;
    }

    /// Returns `Some` once the task has been spawned on an executor;
    /// `None` between [`Task::init`] and the spawn call.
    pub(crate) fn get_executor(&self) -> Option<&ExecutorHandle> {
        self.executor.as_ref()
    }

    /// Install the executor handle. Called once by the executor's spawn path
    /// before the task is pushed onto any queue.
    pub(crate) unsafe fn set_executor(this: *mut Self, executor: ExecutorHandle) {
        unsafe { (*this).executor = Some(executor) };
    }

    /// Whether the task is currently in an executor's ready list.
    pub(crate) fn is_ready_queued(&self) -> bool {
        self.ready_list_link.in_list()
    }

    /// Whether the task is currently in the sleep queue
    pub(crate) fn is_sleep_queued(&self) -> bool {
        self.sleep_list_link.in_list()
    }
}

impl Drop for RawTask {
    fn drop(&mut self) {
        // If the task was spawned, drain its pending-ready list so the
        // AtomicQueueLink can be torn down. A task that was attached but
        // never spawned has no executor and nothing to drain.
        if let Some(executor) = self.executor.as_ref() {
            let raw = unsafe { &*executor.raw() };
            raw.resume_pending_tasks();
        }
    }
}

impl_linked!(ready_list_link, RawTask, TaskReadyListTag);
impl_linked!(sleep_list_link, RawTask, WaitQueueTag);
impl_atomic_linked!(pending_ready_list_link, RawTask, TaskReadyListTag);

pub struct TaskVTable {
    // raw(Task<F>)
    raw: fn(*mut ()) -> *mut RawTask,

    // poll(Task<F>)
    poll: unsafe fn(*mut ()) -> bool,

    // try_read_output(Task<F>, Poll<F::Output>, &Waker)
    try_read_output: unsafe fn(*mut (), *mut (), &Waker) -> (),

    // drop_handle(Task<F>)
    drop_handle: fn(*mut ()) -> (),
}

pub struct Task<F: Future> {
    state: AtomicU32,
    raw: MaybeUninit<RawTask>,
    future: MaybeUninit<F>,
    output: MaybeUninit<F::Output>,
    _pinned: core::marker::PhantomPinned,
}

impl<F: Future> Task<F> {
    pub const INITIALIZER: Task<F> = Task::<F>::new();

    const ASYNC_TASK_VTABLE: TaskVTable = TaskVTable {
        raw: Self::raw,
        poll: Self::vpoll,
        try_read_output: Self::try_read_output,
        drop_handle: Self::drop_handle,
    };

    pub const fn new() -> Task<F> {
        Task {
            state: AtomicU32::new(ASYNC_TASK_STATE_FREE),
            raw: MaybeUninit::uninit(),
            future: MaybeUninit::uninit(),
            output: MaybeUninit::uninit(),
            _pinned: core::marker::PhantomPinned,
        }
    }

    pub fn init(self: Pin<&mut Self>, future: F) -> TaskHandle<F::Output> {
        unsafe {
            let task = Pin::get_unchecked_mut(self);

            let task_ptr = task as *mut _ as *mut ();

            task.raw.write(RawTask {
                executor: None,
                ready_list_link: Node::new(),
                sleep_list_link: Node::new(),
                pending_ready_list_link: AtomicNode::new(),
                waiter: WaitQueueEntry::new(),
                wakeup_time: Instant::ZERO,
                task_ptr,
                poll_fn: Self::vpoll,
                on_result_waker: None,
                _pinned: core::marker::PhantomPinned,
            });

            task.future.write(future);
            task.state
                .store(ASYNC_TASK_STATE_RUNNING, Ordering::Relaxed);

            TaskHandle::new(Pin::new_unchecked(task))
        }
    }

    pub fn claim(&mut self) -> Option<&'_ mut Task<F>> {
        match self.state.compare_exchange(
            ASYNC_TASK_STATE_FREE,
            ASYNC_TASK_STATE_UNINIT,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => Some(self),
            Err(_) => None,
        }
    }

    fn drop_handle(data: *mut ()) {
        let task = unsafe { &*(data as *const Task<F>) };

        // Dropping a handle detaches: a still-running task keeps running on its
        // executor (reached through its queues) and is never reclaimed — its
        // static pool slot stays in use. Only a finished task is safe to
        // reclaim here, since its future has completed and its output was
        // never read by a join.
        if task.state.load(Ordering::Acquire) == ASYNC_TASK_STATE_FINISHED {
            let prev = task.state.swap(ASYNC_TASK_STATE_FREE, Ordering::Release);
            if prev == ASYNC_TASK_STATE_FINISHED {
                // Drop control block, future and output.
                unsafe {
                    task.raw.assume_init_read();
                    task.future.assume_init_read();
                    task.output.assume_init_read();
                }
            }
        }
    }

    fn raw(data: *mut ()) -> *mut RawTask {
        let task = unsafe { &mut *(data as *mut Task<F>) };
        assert!(task.state.load(Ordering::Relaxed) != ASYNC_TASK_STATE_UNINIT);
        task.raw.as_mut_ptr()
    }

    fn poll(&mut self) -> bool {
        match self.state.load(Ordering::Relaxed) {
            ASYNC_TASK_STATE_RUNNING => {}
            _ => return true,
        }

        let raw = unsafe { self.raw.assume_init_mut() };
        let future = unsafe { self.future.assume_init_mut() };
        let future = unsafe { Pin::new_unchecked(future) };

        let waker = raw.waker();

        let mut context = Context::from_waker(&waker);

        match future.poll(&mut context) {
            Poll::Ready(result) => {
                // Write future result to output in AsyncTask
                self.output.write(result);

                self.state
                    .store(ASYNC_TASK_STATE_FINISHED, Ordering::Relaxed);

                // If a task is waiting for this task to join, wake it
                if let Some(waker) = raw.on_result_waker.take() {
                    waker.wake();
                }

                true
            }
            Poll::Pending => false,
        }
    }

    unsafe fn vpoll(task_ptr: *mut ()) -> bool {
        let task = unsafe { &mut *(task_ptr as *mut Task<F>) };
        task.poll()
    }

    unsafe fn try_read_output(data: *mut (), output_ptr: *mut (), waker: &Waker) {
        let task = unsafe { &mut *(data as *mut Task<F>) };
        let output = unsafe { &mut *(output_ptr as *mut Poll<F::Output>) };

        match task.state.load(Ordering::Relaxed) {
            ASYNC_TASK_STATE_FINISHED => {
                // Output is available
                *output = Poll::Ready(unsafe { task.output.assume_init_read() });
                unsafe {
                    task.raw.assume_init_read();
                    task.future.assume_init_read();
                }
                task.state.store(ASYNC_TASK_STATE_FREE, Ordering::Relaxed);
            }
            _ => {
                // Output is not available, register waker
                let control_block = unsafe { task.raw.assume_init_mut() };
                control_block.on_result_waker = Some(waker.clone());
                *output = Poll::Pending;
            }
        }
    }
}

pub struct RawTaskHandle {
    pub(crate) task_ptr: *mut (),
    pub(crate) vtable: &'static TaskVTable,
}

impl RawTaskHandle {
    pub fn as_ref(&self) -> Pin<&'_ RawTask> {
        unsafe { Pin::new_unchecked(&*(self.vtable.raw)(self.task_ptr)) }
    }

    pub fn as_mut(&mut self) -> Pin<&'_ mut RawTask> {
        unsafe { Pin::new_unchecked(&mut *(self.vtable.raw)(self.task_ptr)) }
    }

    /// Pointer to the embedded `RawTask`. Used by executor `spawn` paths to
    /// write the executor handle before queueing.
    pub(crate) fn raw_task_ptr(&self) -> *mut RawTask {
        (self.vtable.raw)(self.task_ptr)
    }

    pub fn poll(&self) -> bool {
        unsafe { (self.vtable.poll)(self.task_ptr) }
    }

    pub(super) unsafe fn try_read_output<T>(&self, output: &mut Poll<T>, waker: &Waker) {
        unsafe {
            (self.vtable.try_read_output)(self.task_ptr, output as *mut Poll<T> as *mut (), waker);
        }
    }
}

impl Drop for RawTaskHandle {
    fn drop(&mut self) {
        (self.vtable.drop_handle)(self.task_ptr);
    }
}

pub struct TaskHandle<T> {
    raw: RawTaskHandle,
    _phantom: PhantomData<T>,
}

impl<T> TaskHandle<T> {
    pub fn new<F: Future>(task: Pin<&mut Task<F>>) -> TaskHandle<T> {
        let task = unsafe { Pin::get_unchecked_mut(task) };
        TaskHandle {
            raw: RawTaskHandle {
                task_ptr: task as *mut _ as *mut _,
                vtable: &Task::<F>::ASYNC_TASK_VTABLE,
            },
            _phantom: PhantomData,
        }
    }

    pub(crate) fn as_raw(&self) -> &'_ RawTaskHandle {
        &self.raw
    }

    pub fn as_ref(&self) -> Pin<&'_ RawTask> {
        self.raw.as_ref()
    }

    pub fn as_mut(&mut self) -> Pin<&'_ mut RawTask> {
        self.raw.as_mut()
    }

    pub fn poll(&self) -> bool {
        self.raw.poll()
    }

    pub fn try_read_output(&self, output: &mut Poll<T>, waker: &Waker) {
        unsafe { self.raw.try_read_output(output, waker) }
    }
}
