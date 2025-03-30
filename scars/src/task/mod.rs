pub mod task;
pub mod task_pool;
use crate::Priority;
use crate::cell::{PinRefCell, RefCell};
use crate::events::{EXECUTOR_WAKEUP_EVENT, wait_events_until};
use crate::kernel::interrupt::InterruptRef;
use crate::kernel::list::LinkedList;
use crate::kernel::waiter::WaitQueueTag;
use crate::kernel::{Scheduler, atomic_queue::AtomicQueue, scheduler::ExecutionContext};
use crate::thread::ThreadRef;
use crate::time::{Duration, Instant};
use crate::tls::LocalStorage;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};
pub use task::{RawTask, Task, TaskReadyListTag};
use task::{RawTaskHandle, TaskHandle};
pub use task_pool::{InitializedTask, TaskPool};

#[macro_export]
macro_rules! make_interrupt_executor {
    ( ) => {{
        static INTERRUPT_EXECUTOR: $crate::tls::LocalCell<$crate::task::InterruptExecutor> =
            $crate::tls::LocalCell::new();
        &INTERRUPT_EXECUTOR
    }};
}

#[macro_export]
macro_rules! make_thread_executor {
    ( ) => {{
        static THREAD_EXECUTOR: $crate::tls::LocalCell<$crate::task::ThreadExecutor> =
            $crate::tls::LocalCell::new();
        &THREAD_EXECUTOR
    }};
}

#[derive(Copy, Clone)]
pub enum LocalExecutor {
    None,
    Thread(&'static ThreadExecutor),
    Interrupt(&'static InterruptExecutor),
}

impl LocalExecutor {
    pub fn get() -> Self {
        match Scheduler::current_execution_context() {
            ExecutionContext::Thread(_) => {
                let executor = LocalStorage::get::<ThreadExecutor>().unwrap();
                LocalExecutor::Thread(executor)
            }
            ExecutionContext::Interrupt(_) => {
                let executor = LocalStorage::get::<InterruptExecutor>().unwrap();
                LocalExecutor::Interrupt(executor)
            }
        }
    }

    pub fn task_sleep_until(&self, task: Pin<&mut RawTask>, deadline: Instant) {
        match self {
            LocalExecutor::None => panic!("No executor found"),
            LocalExecutor::Thread(executor) => executor.task_sleep_until(task, deadline),
            LocalExecutor::Interrupt(executor) => executor.task_sleep_until(task, deadline),
        }
    }

    pub fn spawn<T>(&self, task: InitializedTask<T>) -> JoinHandle<T> {
        match self {
            LocalExecutor::None => panic!("No executor found"),
            LocalExecutor::Thread(executor) => executor.spawn(task),
            LocalExecutor::Interrupt(executor) => executor.spawn(task),
        }
    }

    pub fn priority(&self) -> Priority {
        match self {
            LocalExecutor::None => panic!("No executor found"),
            LocalExecutor::Thread(executor) => executor.priority(),
            LocalExecutor::Interrupt(executor) => executor.priority(),
        }
    }

    pub fn resume_task(&'static self, task: Pin<&RawTask>) {
        match self {
            LocalExecutor::None => panic!("No executor found"),
            LocalExecutor::Thread(executor) => executor.resume_task(task),
            LocalExecutor::Interrupt(executor) => executor.resume_task(task),
        }
    }

    fn resume_pending_tasks(&self) {
        match self {
            LocalExecutor::None => panic!("No executor found"),
            LocalExecutor::Thread(executor) => executor.resume_pending_tasks(),
            LocalExecutor::Interrupt(executor) => executor.resume_pending_tasks(),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum PollKind {
    OnAttach,
    OnInterrupt,
    OnWakeup,
    OnResume,
}

struct RawInterruptExecutor {
    interrupt: InterruptRef,
    ready_queue: PinRefCell<LinkedList<RawTask, TaskReadyListTag>>,
    sleep_queue: PinRefCell<LinkedList<RawTask, WaitQueueTag>>,

    /// Tasks waiting for the interrupt to occur
    wfi_queue: PinRefCell<LinkedList<RawTask, WaitQueueTag>>,

    pending_ready_queue: AtomicQueue<RawTask, TaskReadyListTag>,
}

impl RawInterruptExecutor {
    pub fn new(interrupt: InterruptRef) -> RawInterruptExecutor {
        RawInterruptExecutor {
            interrupt,
            ready_queue: PinRefCell::new(LinkedList::new()),
            sleep_queue: PinRefCell::new(LinkedList::new()),
            wfi_queue: PinRefCell::new(LinkedList::new()),
            pending_ready_queue: AtomicQueue::new(),
        }
    }

    fn poll(&'static self, kind: PollKind) {
        match kind {
            PollKind::OnAttach => {
                self.resume_wfi_tasks();
                self.resume_sleeping_tasks();
            }
            PollKind::OnInterrupt => {
                self.resume_wfi_tasks();
            }
            PollKind::OnWakeup => {
                self.resume_sleeping_tasks();
            }
            PollKind::OnResume => {}
        }

        self.resume_pending_tasks();
        self.poll_ready_tasks();

        // Put interrupt handler to kernel wakeup queue if there are sleeping tasks
        let deadline_opt = Pin::static_ref(&self.sleep_queue)
            .borrow()
            .as_ref()
            .head()
            .map(|task| task.wakeup_time);

        if let Some(deadline) = deadline_opt {
            self.schedule_wakeup(deadline);
        }
    }

    fn poll_ready_tasks(&'static self) {
        while let Some(ready_task) = Pin::static_ref(&self.ready_queue)
            .borrow_mut()
            .as_mut()
            .pop_front()
        {
            ready_task.poll();
        }
    }

    fn schedule_wakeup(&'static self, wakeup_time: Instant) {
        Scheduler::schedule_interrupt_wakeup(unsafe { self.interrupt.as_ref() }, wakeup_time.tick);
    }

    /// Set the interrupt executor to be polled when the outermost interrupt handler returns.
    /// This should be called when executor poll is requested in interrupt context.
    fn set_poll_pending(&'static self) {
        self.interrupt.set_pending_executor_poll()
    }

    /// Execute a syscall that immediately polls the interrupt executor.
    /// This should be called when executor poll is requested in thread context.
    fn poll_now(&'static self) {
        crate::syscall::poll_interrupt_executor(unsafe { self.interrupt.as_ref() })
    }

    // Safe to call from ISR or another thread
    // Note: This method must be called for the task's executor only
    pub(crate) fn resume_task(&'static self, task: Pin<&RawTask>) {
        // Add task to the pending ready queue of the interrupt executor
        self.pending_ready_queue.push_back(task);
        if let LocalExecutor::Interrupt(task_executor) = task.executor {
            assert!(&task_executor.raw as *const _ == self as *const _);

            // Wake up the interrupt executor depending on the current execution context
            match Scheduler::current_execution_context() {
                ExecutionContext::Interrupt(_) => {
                    // By setting the executor wakeup pending, the task will be polled
                    // when the outermost interrupt handler returns.
                    self.set_poll_pending();
                }
                ExecutionContext::Thread(_) => {
                    self.poll_now();
                }
            }
        } else {
            unreachable!();
        }
    }

    fn task_sleep_until(&'static self, mut task: Pin<&mut RawTask>, deadline: Instant) {
        task.as_mut().set_wakeup_time(deadline);
        Pin::static_ref(&self.sleep_queue)
            .borrow_mut()
            .as_mut()
            .insert_after(task.into_ref(), |queue_task| {
                queue_task.wakeup_time <= deadline
            });
    }

    pub(crate) fn task_wait_for_interrupt(&'static self, task: Pin<&mut RawTask>) {
        Pin::static_ref(&self.wfi_queue)
            .borrow_mut()
            .as_mut()
            .push_back(task.into_ref());
    }

    pub fn resume_pending_tasks(&'static self) {
        while let Some(pending_ready_task) = self.pending_ready_queue.pop_front() {
            Pin::static_ref(&self.ready_queue)
                .borrow_mut()
                .as_mut()
                .push_back(pending_ready_task);
        }
    }

    fn resume_sleeping_tasks(&'static self) {
        // Resume sleeping tasks that should be woken up
        let now = Instant::now();
        let mut sleep_queue = Pin::static_ref(&self.sleep_queue).borrow_mut();
        loop {
            let head_wakeup_time_opt = sleep_queue.as_ref().head().map(|task| task.wakeup_time);
            if let Some(head_wakeup_time) = head_wakeup_time_opt {
                if head_wakeup_time <= now {
                    let task = sleep_queue.as_mut().pop_front().unwrap();
                    Pin::static_ref(&self.ready_queue)
                        .borrow_mut()
                        .as_mut()
                        .push_back(task);
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    fn resume_wfi_tasks(&'static self) {
        // Resume tasks waiting for the interrupt to occur
        let mut wfi_queue = Pin::static_ref(&self.wfi_queue).borrow_mut();
        let mut ready_queue = Pin::static_ref(&self.ready_queue).borrow_mut();

        loop {
            if let Some(task) = wfi_queue.as_mut().pop_front() {
                ready_queue.as_mut().push_back(task);
            } else {
                break;
            }
        }
    }

    fn spawn(&'static self, task_handle: &RawTaskHandle) {
        let task_to_spawn = task_handle.as_ref();
        Pin::static_ref(&self.ready_queue)
            .borrow_mut()
            .as_mut()
            .push_back(task_to_spawn);
    }

    fn priority(&self) -> Priority {
        self.interrupt.base_priority()
    }
}

pub struct InterruptExecutor {
    raw: RawInterruptExecutor,
    // To make sure that Executor is not Send or Sync
    _phantom: PhantomData<*const ()>,
}

impl InterruptExecutor {
    pub fn new(interrupt: InterruptRef) -> InterruptExecutor {
        InterruptExecutor {
            raw: RawInterruptExecutor::new(interrupt),
            _phantom: PhantomData,
        }
    }

    pub fn priority(&self) -> Priority {
        self.raw.priority()
    }

    pub fn spawn<T>(&'static self, task: InitializedTask<T>) -> JoinHandle<T> {
        let task_handle = task.with_executor(LocalExecutor::Interrupt(self));
        self.raw.spawn(task_handle.as_raw());
        JoinHandle::new(task_handle)
    }

    pub fn poll(&'static self, kind: PollKind) {
        self.raw.poll(kind)
    }

    fn resume_task(&'static self, task: Pin<&RawTask>) {
        self.raw.resume_task(task);
    }

    fn task_sleep_until(&'static self, task: Pin<&mut RawTask>, deadline: Instant) {
        self.raw.task_sleep_until(task, deadline);
    }

    fn resume_pending_tasks(&'static self) {
        self.raw.resume_pending_tasks()
    }

    pub(crate) fn task_wait_for_interrupt(&'static self, task: Pin<&mut RawTask>) {
        self.raw.task_wait_for_interrupt(task);
    }
}

struct RawThreadExecutor {
    thread: ThreadRef,
    ready_queue: PinRefCell<LinkedList<RawTask, TaskReadyListTag>>,
    sleep_queue: PinRefCell<LinkedList<RawTask, WaitQueueTag>>,
    pending_ready_queue: AtomicQueue<RawTask, TaskReadyListTag>,
}

impl RawThreadExecutor {
    pub const fn new(thread: ThreadRef) -> RawThreadExecutor {
        RawThreadExecutor {
            thread,
            ready_queue: PinRefCell::new(LinkedList::new()),
            sleep_queue: PinRefCell::new(LinkedList::new()),
            pending_ready_queue: AtomicQueue::new(),
        }
    }

    fn spawn(&'static self, task_handle: &RawTaskHandle) {
        let task_to_spawn = task_handle.as_ref();
        Pin::static_ref(&self.ready_queue)
            .borrow_mut()
            .as_mut()
            .push_back(task_to_spawn);
    }

    fn block_on(&'static self, task_handle: &RawTaskHandle) -> bool {
        let block_on_task = task_handle.as_ref();
        Pin::static_ref(&self.ready_queue)
            .borrow_mut()
            .as_mut()
            .push_back(block_on_task.as_ref());

        loop {
            self.resume_sleeping_tasks();
            self.resume_pending_tasks(false);

            let mut ready_queue = Pin::static_ref(&self.ready_queue).borrow_mut();

            // Poll ready tasks
            while let Some(ready_task) = ready_queue.as_mut().pop_front() {
                if ready_task.poll() {
                    if &*ready_task as *const _ == &*block_on_task as *const _ {
                        return true;
                    }
                }
            }

            let deadline_opt = Pin::static_ref(&self.sleep_queue)
                .borrow()
                .as_ref()
                .head()
                .map(|task| task.wakeup_time);
            let _ = wait_events_until(EXECUTOR_WAKEUP_EVENT, deadline_opt);
        }
    }

    fn priority(&self) -> Priority {
        self.thread.priority()
    }

    // Safe to call from ISR or another thread
    fn resume_task(&'static self, task: Pin<&RawTask>) {
        self.pending_ready_queue.push_back(task);
        self.thread.send_events(EXECUTOR_WAKEUP_EVENT);
    }

    // Safe to call only from the local thread
    fn task_sleep_until(&'static self, mut task: Pin<&mut RawTask>, deadline: Instant) {
        task.as_mut().set_wakeup_time(deadline);
        Pin::static_ref(&self.sleep_queue)
            .borrow_mut()
            .as_mut()
            .insert_after(task.into_ref(), |queue_task| {
                queue_task.wakeup_time <= deadline
            });
    }

    // Safe to call only from the local thread
    fn resume_pending_tasks(&'static self, notify_executor: bool) {
        let mut task_became_ready: bool = false;
        let mut ready_queue = Pin::static_ref(&self.ready_queue).borrow_mut();

        while let Some(pending_ready_task) = self.pending_ready_queue.pop_front() {
            ready_queue.as_mut().push_back(pending_ready_task);
            task_became_ready = true;
        }

        if task_became_ready & notify_executor {
            self.thread.send_events(EXECUTOR_WAKEUP_EVENT);
        }
    }

    fn resume_sleeping_tasks(&'static self) {
        let mut ready_queue = Pin::static_ref(&self.ready_queue).borrow_mut();
        let mut sleep_queue = Pin::static_ref(&self.sleep_queue).borrow_mut();

        // Resume sleeping tasks that should be woken up
        loop {
            let now = Instant::now();
            if let Some(head) = sleep_queue.as_ref().head() {
                if head.wakeup_time <= now {
                    let task = sleep_queue.as_mut().pop_front().unwrap();
                    ready_queue.as_mut().push_back(task);
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }
}

pub struct ThreadExecutor {
    raw: RawThreadExecutor,
    // To make sure that Executor is not Send or Sync
    _phantom: PhantomData<*const ()>,
}

impl ThreadExecutor {
    pub const fn new(thread: ThreadRef) -> ThreadExecutor {
        ThreadExecutor {
            raw: RawThreadExecutor::new(thread),
            _phantom: PhantomData,
        }
    }

    pub fn spawn<T>(&'static self, task: InitializedTask<T>) -> JoinHandle<T> {
        let task_handle = task.with_executor(LocalExecutor::Thread(self));
        self.raw.spawn(task_handle.as_raw());
        JoinHandle::new(task_handle)
    }

    pub fn block_on<T>(&'static self, task_handle: TaskHandle<T>) -> T {
        let mut output = Poll::Pending;
        let _ = self.raw.block_on(task_handle.as_raw());
        task_handle.try_read_output(&mut output, core::task::Waker::noop());
        match output {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("Task was not ready after block_on"),
        }
    }

    fn priority(&self) -> Priority {
        self.raw.priority()
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
}

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

        let executor = LocalStorage::get::<ThreadExecutor>().unwrap();
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
    task_pool: &'static mut TaskPool<F, N>,
    future: F,
) -> Result<JoinHandle<F::Output>, ()> {
    let executor = LocalExecutor::get();
    match task_pool.alloc() {
        Some(task) => {
            let initialized_task = task.attach(|| future);
            Ok(executor.spawn(initialized_task))
        }
        None => Err(()),
    }
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    let executor = LocalStorage::get::<ThreadExecutor>().unwrap();
    let pinned_task = core::pin::pin!(Task::new());
    let task_handle = pinned_task.init(future, LocalExecutor::Thread(executor));
    executor.block_on(task_handle)
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
            let executor = LocalExecutor::get();
            let task = unsafe { &mut *(cx.waker().data() as *mut RawTask) };
            let pinned_task = unsafe { Pin::new_unchecked(task) };
            executor.task_sleep_until(pinned_task, this.deadline);
            Poll::Pending
        }
    }
}
