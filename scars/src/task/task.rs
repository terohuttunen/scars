use super::LocalExecutor;
use crate::kernel::atomic_list::*;
use crate::kernel::list::{impl_linked, LinkedListTag, Node};
use crate::kernel::waiter::{Suspendable, WaitQueueTag};
use crate::time::Instant;
use core::future::Future;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

pub struct TaskReadyListTag;

impl LinkedListTag for TaskReadyListTag {}

pub const ASYNC_TASK_STATE_FREE: u32 = 0;
pub const ASYNC_TASK_STATE_UNINIT: u32 = 1;
pub const ASYNC_TASK_STATE_RUNNING: u32 = 2;
pub const ASYNC_TASK_STATE_FINISHED: u32 = 3;

pub struct RawTask {
    pub(crate) executor: LocalExecutor,
    ready_list_link: Node<RawTask, TaskReadyListTag>,
    sleep_list_link: Node<RawTask, WaitQueueTag>,
    pending_ready_list_link: AtomicNode<RawTask, TaskReadyListTag>,
    pub(crate) waiter: Suspendable,
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
        raw.executor.resume_task(unsafe { Pin::new_unchecked(raw) });
    }

    fn waker_drop(_data: *const ()) {}

    pub fn set_wakeup_time(self: Pin<&mut Self>, wakeup_time: Instant) {
        let task = unsafe { Pin::get_unchecked_mut(self) };
        task.wakeup_time = wakeup_time;
    }
}

impl Drop for RawTask {
    fn drop(&mut self) {
        // Readying pending tasks will free the task from the atomic pending_ready_tasks
        // list if it is there, allowing safe dropping of the AtomicQueueLink.
        self.executor.resume_pending_tasks();
    }
}

impl_linked!(ready_list_link, RawTask, TaskReadyListTag);
impl_linked!(sleep_list_link, RawTask, WaitQueueTag);
impl_atomic_linked!(pending_ready_list_link, RawTask, TaskReadyListTag);

pub struct TaskVTable {
    // control_block(AsyncTask<F>)
    control_block: fn(*mut ()) -> *mut RawTask,

    // poll(AsyncTask<F>)
    poll: unsafe fn(*mut ()) -> bool,

    // try_read_output(AsyncTask<F>, Poll<F::Output>, &Waker)
    try_read_output: unsafe fn(*mut (), *mut (), &Waker) -> (),

    // drop_handle(AsyncTask<F>)
    drop_handle: fn(*mut ()) -> (),
}

pub struct Task<F: Future> {
    state: AtomicU32,
    control_block: MaybeUninit<RawTask>,
    future: MaybeUninit<F>,
    output: MaybeUninit<F::Output>,
    _pinned: core::marker::PhantomPinned,
}

impl<F: Future> Task<F> {
    pub const INITIALIZER: Task<F> = Task::<F>::new();

    const ASYNC_TASK_VTABLE: TaskVTable = TaskVTable {
        control_block: Self::control_block,
        poll: Self::vpoll,
        try_read_output: Self::try_read_output,
        drop_handle: Self::drop_handle,
    };

    pub const fn new() -> Task<F> {
        Task {
            state: AtomicU32::new(ASYNC_TASK_STATE_FREE),
            control_block: MaybeUninit::uninit(),
            future: MaybeUninit::uninit(),
            output: MaybeUninit::uninit(),
            _pinned: core::marker::PhantomPinned,
        }
    }

    pub fn init(self: Pin<&mut Self>, future: F, executor: LocalExecutor) -> TaskHandle<F::Output> {
        unsafe {
            let task = Pin::get_unchecked_mut(self);

            let waker = Waker::from_raw(RawWaker::new(
                task.control_block.as_ptr() as *const (),
                &RawTask::TASK_WAKER_VTABLE,
            ));

            let task_ptr = task as *mut _ as *mut ();

            // The task inherits current executor task priority
            let priority = executor.priority();

            task.control_block.write(RawTask {
                executor,
                ready_list_link: Node::new(),
                sleep_list_link: Node::new(),
                pending_ready_list_link: AtomicNode::new(),
                waiter: Suspendable::new_async(priority, waker),
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
        let old_state = task.state.swap(ASYNC_TASK_STATE_FREE, Ordering::Release);

        // If task is in the ready queue, first set its state so that is will
        // be dropped when it is removed from the ready queue.
        match old_state {
            ASYNC_TASK_STATE_RUNNING => {
                // Drop control block and future
                unsafe {
                    task.control_block.assume_init_read();
                    task.future.assume_init_read();
                }
            }
            ASYNC_TASK_STATE_FINISHED => {
                // Drop control block, future and output
                unsafe {
                    task.control_block.assume_init_read();
                    task.future.assume_init_read();
                    task.output.assume_init_read();
                }
            }
            _ => {
                // Nothing to drop
            }
        }
    }

    fn control_block(data: *mut ()) -> *mut RawTask {
        let task = unsafe { &mut *(data as *mut Task<F>) };
        assert!(task.state.load(Ordering::Relaxed) != ASYNC_TASK_STATE_UNINIT);
        task.control_block.as_mut_ptr()
    }

    fn poll(&mut self) -> bool {
        let control_block = unsafe { self.control_block.assume_init_mut() };
        let future = unsafe { self.future.assume_init_mut() };
        let future = unsafe { Pin::new_unchecked(future) };

        let waker = control_block.waker();

        let mut context = Context::from_waker(&waker);

        match future.poll(&mut context) {
            Poll::Ready(result) => {
                // Write future result to output in AsyncTask
                self.output.write(result);

                self.state
                    .store(ASYNC_TASK_STATE_FINISHED, Ordering::Relaxed);

                // If a task is waiting for this task to join, wake it
                if let Some(waker) = control_block.on_result_waker.take() {
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
                    task.control_block.assume_init_read();
                    task.future.assume_init_read();
                }
                task.state.store(ASYNC_TASK_STATE_FREE, Ordering::Relaxed);
            }
            _ => {
                // Output is not available, register waker
                let control_block = unsafe { task.control_block.assume_init_mut() };
                control_block.on_result_waker = Some(waker.clone());
                *output = Poll::Pending;
            }
        }
    }
}

pub(super) struct RawTaskHandle {
    pub(crate) task_ptr: *mut (),
    pub(crate) vtable: &'static TaskVTable,
}

impl RawTaskHandle {
    pub fn as_ref(&self) -> Pin<&'_ RawTask> {
        unsafe { Pin::new_unchecked(&*(self.vtable.control_block)(self.task_ptr)) }
    }

    pub fn poll(&self) -> bool {
        unsafe { (self.vtable.poll)(self.task_ptr) }
    }

    pub(super) unsafe fn try_read_output<T>(&self, output: &mut Poll<T>, waker: &Waker) {
        (self.vtable.try_read_output)(self.task_ptr, output as *mut Poll<T> as *mut (), waker);
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

    pub(super) fn as_raw(&self) -> &'_ RawTaskHandle {
        &self.raw
    }

    pub fn as_ref(&self) -> Pin<&'_ RawTask> {
        self.raw.as_ref()
    }

    pub fn poll(&self) -> bool {
        self.raw.poll()
    }

    pub fn try_read_output(&self, output: &mut Poll<T>, waker: &Waker) {
        unsafe { self.raw.try_read_output(output, waker) }
    }
}
