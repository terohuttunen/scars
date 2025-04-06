use crate::cell::LockedCell;
use crate::events::EXECUTOR_WAKEUP_EVENT;
pub use crate::events::{REQUIRE_ALL_EVENTS, TryWaitEventsError};
use crate::kernel::list::{LinkedList, LinkedListTag};
use crate::kernel::priority::{
    AtomicPriorityPair, AtomicPriorityStatusPair, INVALID_PRIORITY, InterruptPriority, Priority,
    PriorityStatus,
};
use crate::kernel::scheduler::{ExecutionContext, Scheduler};
use crate::kernel::tracing;
use crate::kernel::waiter::Suspendable;
use crate::sync::{
    OnceLock,
    ceiling_lock::RawCeilingLock,
    interrupt_lock::{InterruptLock, InterruptLockKey},
};
use crate::task::{
    InitializedTask, InterruptExecutor, JoinHandle, PollKind, RawTask, TaskPool, ThreadExecutor,
};
use crate::thread::{LockListTag, RawThread};
use crate::tls::{LocalCell, LocalStorage, LocalStorageArray};
pub use critical_section::CriticalSection;
use scars_khal::*;

use crate::kernel::hal::{
    Context, MAX_INTERRUPT_NUMBER, claim_interrupt, complete_interrupt,
    enable_interrupt, get_interrupt_priority, get_interrupt_threshold,
    set_interrupt_priority, set_interrupt_threshold,
};
use core::cell::UnsafeCell;
use core::future::{Future, poll_fn};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::ptr::{NonNull, addr_of, addr_of_mut};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use core::task::Poll;

use super::atomic_queue::{AtomicNode, AtomicQueue, impl_atomic_linked};
use super::list::LinkedListNode;

use static_cell::ConstStaticCell;

pub struct PendingNotifyTag;

impl LinkedListTag for PendingNotifyTag {}

pub type InterruptNumber = u16;

#[macro_export]
macro_rules! make_interrupt_handler {
    ($intnum: expr, $prio : expr, $local_storage_size : expr, executor = true) => {{
        let mut handler = $crate::make_interrupt_handler!($intnum, $prio, $local_storage_size);
        let executor = $crate::make_interrupt_executor!();
        handler.start_executor(executor);
        handler
    }};
    ($intnum: expr, $prio : expr, $local_storage_size : expr) => {{
        let mut handler = $crate::make_interrupt_handler!($intnum, $prio);
        let local_storage = $crate::make_local_storage!($local_storage_size);
        handler.set_local_storage(local_storage);
        handler
    }};
    ($intnum: expr, $prio : expr) => {{
        type T = impl ::core::marker::Sized + ::core::marker::Send + FnMut();
        static HANDLER: $crate::kernel::interrupt::InterruptHandler<{ $prio }, T> =
            $crate::kernel::interrupt::InterruptHandler::new($intnum);
        HANDLER.init()
    }};
}

unsafe extern "C" {
    static mut _isr_stack_start: u8;
}

static CURRENT_INTERRUPT_CONTROL_BLOCK: AtomicPtr<RawInterruptHandler> =
    AtomicPtr::new(core::ptr::null_mut());

static PENDING_INTERRUPT_EXECUTOR_POLLS: AtomicQueue<RawInterruptHandler, PendingNotifyTag> =
    AtomicQueue::new();

pub(crate) fn switch_current_interrupt(
    icb_ptr: *mut RawInterruptHandler,
) -> *mut RawInterruptHandler {
    CURRENT_INTERRUPT_CONTROL_BLOCK.swap(icb_ptr, Ordering::SeqCst)
}

pub(crate) fn restore_current_interrupt(icb_ptr: *mut RawInterruptHandler) {
    CURRENT_INTERRUPT_CONTROL_BLOCK.store(icb_ptr, Ordering::SeqCst)
}

pub(crate) fn current_interrupt() -> Option<NonNull<RawInterruptHandler>> {
    NonNull::new(CURRENT_INTERRUPT_CONTROL_BLOCK.load(Ordering::SeqCst))
}

pub(crate) fn set_ceiling_threshold(ceiling: PriorityStatus) {
    match ceiling {
        PriorityStatus::Interrupt(prio) => set_interrupt_threshold(prio),
        PriorityStatus::Thread(_) | PriorityStatus::Invalid => set_interrupt_threshold(0),
    }
}

#[repr(align(16))]
#[repr(C)]
pub struct RawInterruptHandler {
    intnum: InterruptNumber,
    base_priority: Priority,

    closure_ptr: *const (),

    lock_priorities: AtomicPriorityStatusPair,

    // The kernel must keep track of owned ceiling locks also in interrupt handlers,
    // because the locks might be released in any order.
    owned_locks: UnsafeCell<LinkedList<RawCeilingLock, LockListTag>>,

    pending_interrupt_executor_poll_link: AtomicNode<RawInterruptHandler, PendingNotifyTag>,

    pub(crate) local_storage: OnceLock<LocalStorage>,

    pub(crate) suspendable: Suspendable,
}

impl_atomic_linked!(
    pending_interrupt_executor_poll_link,
    RawInterruptHandler,
    PendingNotifyTag
);

unsafe impl Sync for RawInterruptHandler {}

impl RawInterruptHandler {
    pub(crate) const fn new(intnum: InterruptNumber, prio: Priority) -> RawInterruptHandler {
        RawInterruptHandler {
            intnum,
            base_priority: prio,
            closure_ptr: core::ptr::null(),
            lock_priorities: AtomicPriorityStatusPair::new((
                PriorityStatus::valid(prio),
                PriorityStatus::invalid(),
            )),
            owned_locks: UnsafeCell::new(LinkedList::new()),
            pending_interrupt_executor_poll_link: AtomicNode::new(),
            local_storage: OnceLock::new(),
            suspendable: Suspendable::new(),
        }
    }

    pub(crate) fn set_local_storage(
        &self,
        local_storage: LocalStorage,
    ) -> Result<(), LocalStorage> {
        self.local_storage.set(local_storage)
    }

    /// SAFETY: Caller must guarantee that the lock is free, and that it will be released.
    pub(crate) unsafe fn acquire_lock(self: Pin<&Self>, lock: Pin<&RawCeilingLock>) {
        let locks = unsafe { Pin::new_unchecked(&mut *(self.owned_locks.get())) };
        locks.insert_after(lock, |list_lock| {
            list_lock.ceiling_priority > lock.ceiling_priority
        });

        self.update_owned_lock_priority();
    }

    /// SAFETY: Caller must guarantee that lock has been acquired by the interrupt handler
    pub(crate) unsafe fn release_lock(&self, lock: Pin<&RawCeilingLock>) {
        let locks = unsafe { Pin::new_unchecked(&mut *(self.owned_locks.get())) };
        locks.remove(lock);

        self.update_owned_lock_priority();
    }

    // SAFETY: Caller must guarantee that handler_ptr points to valid function f(arg)
    // and correct alignment of arg_ptr
    pub unsafe fn attach(
        &mut self,
        handler_ptr: *const (),
        closure_ptr: *const (),
        key: InterruptLockKey<'_>,
    ) {
        self.closure_ptr = closure_ptr;
        set_interrupt_priority(self.intnum, self.base_priority.get_value());
        set_interrupt_vector(self.intnum, key, InterruptVector {
            handler_ptr,
            icb_ptr: self as *const _,
        });

        // TODO: this should use some shared code
        let old_priority = self.raise_nesting_lock_priority(self.base_priority);
        unsafe {
            interrupt_context(self as *const _ as *mut _, || {
                if let Some(executor) = LocalStorage::get::<InterruptExecutor>() {
                    executor.poll(PollKind::OnAttach)
                }
            });
        }
        self.set_nesting_lock_priority(old_priority);
    }

    pub fn is_attached(&self, _key: InterruptLockKey<'_>) -> bool {
        !self.closure_ptr.is_null()
    }

    pub fn enable_interrupt(&self, _key: InterruptLockKey<'_>) {
        enable_interrupt(self.intnum);
    }

    // Returns previous priority
    pub(crate) fn raise_nesting_lock_priority(&self, new_priority: Priority) -> PriorityStatus {
        let new_priority = new_priority.max(self.base_priority).into();
        loop {
            let current_priorities = self.lock_priorities.load(Ordering::SeqCst);
            let new_priorities = (current_priorities.0.max(new_priority), current_priorities.1);

            if let Ok(_) = self.lock_priorities.compare_exchange(
                current_priorities,
                new_priorities,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                let ceiling = new_priorities.0.max(new_priorities.1);
                set_ceiling_threshold(ceiling);
                return current_priorities.0;
            }
        }
    }

    pub(crate) fn set_nesting_lock_priority(&self, new_priority: PriorityStatus) {
        let new_priority = new_priority.into();
        loop {
            let current_priorities = self.lock_priorities.load(Ordering::SeqCst);
            let new_priorities = (new_priority, current_priorities.1);

            if let Ok(_) = self.lock_priorities.compare_exchange(
                current_priorities,
                new_priorities,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                let ceiling = new_priorities.0.max(new_priorities.1);
                set_ceiling_threshold(ceiling);
                break;
            }
        }
    }

    fn update_owned_lock_priority(&self) {
        let locks = unsafe { Pin::new_unchecked(&mut *(self.owned_locks.get())) };
        let lock_priority = if let Some(head) = locks.as_ref().head() {
            PriorityStatus::from(head.ceiling_priority)
        } else {
            PriorityStatus::invalid()
        };

        if let Ok((raised_priority, _)) = self.lock_priorities.fetch_update(
            Ordering::SeqCst,
            Ordering::SeqCst,
            |(raised_priority, _)| Some((raised_priority, lock_priority)),
        ) {
            let ceiling_priority = if lock_priority.is_valid() {
                PriorityStatus::max(raised_priority, lock_priority)
            } else {
                raised_priority
            };

            set_ceiling_threshold(ceiling_priority);
        }
    }

    pub fn base_priority(&self) -> Priority {
        self.base_priority
    }

    /// Highest lock priority. Returns Invalid priority if no locks owned by the interrupt.
    #[allow(dead_code)]
    pub(crate) fn lock_priority<'key>(&self) -> PriorityStatus {
        let priorities = self.lock_priorities.load(Ordering::SeqCst);
        priorities.0.max(priorities.1)
    }

    #[allow(dead_code)]
    pub(crate) fn active_priority(&self) -> Priority {
        let lock_priority = self.lock_priority();
        self.base_priority.max_valid(lock_priority)
    }

    pub(crate) fn set_pending_executor_poll(&self) {
        if self.pending_interrupt_executor_poll_link.is_linked() {
            // Poll already pending
            return;
        }

        PENDING_INTERRUPT_EXECUTOR_POLLS.push_back(unsafe { Pin::new_unchecked(self) });
    }

    pub(crate) fn poll_executor(&self) {
        // Raise priority to interrupt's priority, as it would be when polled due to interrupt.
        let old_priority = self.raise_nesting_lock_priority(self.base_priority);

        // Polling is done within the interrupt's context.
        unsafe {
            interrupt_context(self as *const _ as *mut _, || {
                // Current interrupt context is now set, so LocalStorage::get will access the local storage
                // of the awaken interrupt.
                if let Some(executor) = LocalStorage::get::<InterruptExecutor>() {
                    // TODO: when on_wakeup?
                    executor.poll(PollKind::OnWakeup)
                }
            });
        }

        self.set_nesting_lock_priority(old_priority);
    }

    pub fn local_storage(&self) -> Option<&LocalStorage> {
        self.local_storage.get()
    }

    pub fn local_storage_mut(&mut self) -> Option<&mut LocalStorage> {
        self.local_storage.get_mut()
    }

    pub fn as_ptr(&self) -> *const RawInterruptHandler {
        self as *const _
    }

    pub fn start_executor(&mut self, executor: &'static LocalCell<InterruptExecutor>) {
        let interrupt_ref = unsafe { InterruptRef::from_ptr(self.as_ptr()) };
        self.local_storage_mut()
            .unwrap()
            .raw_put_init_with(executor, || InterruptExecutor::new(interrupt_ref));
    }
}

pub async fn wait_for_interrupt() {
    match Scheduler::current_execution_context() {
        ExecutionContext::Interrupt(interrupt) => {
            let mut first_poll = true;
            poll_fn(|cx| {
                let executor = unsafe {
                    interrupt
                        .local_storage()
                        .unwrap()
                        .raw_get::<InterruptExecutor>()
                        .unwrap()
                };
                let task = unsafe { &mut *(cx.waker().data() as *mut RawTask) };
                let pinned_task = unsafe { Pin::new_unchecked(task) };
                if first_poll {
                    executor.task_wait_for_interrupt(pinned_task);
                    first_poll = false;
                    Poll::Pending
                } else {
                    // After the task has been put to WFI queue, it will not be polled
                    // again until it has been transferred from the WFI queue to ready
                    // queue when the interrupt is triggered.
                    Poll::Ready(())
                }
            })
            .await
        }
        ExecutionContext::Thread(_) => {
            panic!("cannot wait for interrupt in thread context")
        }
    }
}

pub struct InterruptBuilder<const PRIO: Priority, F: FnMut() + Send + 'static> {
    handler: &'static mut RawInterruptHandler,
    closure: &'static mut MaybeUninit<F>,
}

impl<const PRIO: Priority, F: FnMut() + Send + 'static> InterruptBuilder<PRIO, F> {
    /// Spawns a future in the interrupt context. The future will be polled
    /// when the interrupt is triggered.
    pub fn spawn<T>(&mut self, task: InitializedTask<T>) -> Result<JoinHandle<T>, ()> {
        match self
            .handler
            .local_storage_mut()
            .and_then(|local_storage| unsafe { local_storage.raw_get::<InterruptExecutor>() })
        {
            Some(executor) => Ok(executor.spawn(task)),
            None => Err(()),
        }
    }

    pub fn set_local_storage(&mut self, local_storage: LocalStorage) {
        self.handler
            .set_local_storage(local_storage)
            .unwrap_or_else(|_| {
                panic!("Local storage already set for interrupt handler");
            });
    }

    pub fn start_executor(&mut self, executor: &'static LocalCell<InterruptExecutor>) {
        self.handler.start_executor(executor);
    }

    pub fn modify<R>(&mut self, f: impl FnOnce(&mut RawInterruptHandler) -> R) -> R {
        f(self.handler)
    }

    pub fn attach<C: FnOnce() -> F>(self, closure: C) -> InitializedInterruptHandler {
        let closure = closure();
        let closure_ref = self.closure.write(closure);
        let closure_ptr = closure_ref as *const F as *const ();
        InterruptLock::with(|key| unsafe {
            self.handler.attach(
                InterruptHandler::<PRIO, F>::closure_wrapper as *const (),
                closure_ptr,
                key,
            )
        });
        InitializedInterruptHandler {
            handler: self.handler,
        }
    }

    pub fn local_storage(&self) -> Option<&LocalStorage> {
        self.handler.local_storage.get()
    }

    pub fn local_storage_mut(&mut self) -> Option<&mut LocalStorage> {
        self.handler.local_storage.get_mut()
    }
}

pub struct InitializedInterruptHandler {
    handler: &'static mut RawInterruptHandler,
}

impl InitializedInterruptHandler {
    pub fn spawn<T>(&self, task: InitializedTask<T>) -> Result<JoinHandle<T>, ()> {
        match self
            .handler
            .local_storage()
            .and_then(|local_storage| unsafe { local_storage.raw_get::<InterruptExecutor>() })
        {
            Some(executor) => Ok(executor.spawn(task)),
            None => Err(()),
        }
    }

    pub fn set_local_storage(&self, local_storage: LocalStorage) {
        self.handler
            .set_local_storage(local_storage)
            .unwrap_or_else(|_| {
                panic!("Local storage already set for interrupt handler");
            });
    }

    pub fn modify<R>(&mut self, f: impl FnOnce(&mut RawInterruptHandler) -> R) -> R {
        f(self.handler)
    }

    pub fn start_executor(&mut self, executor: &'static LocalCell<InterruptExecutor>) {
        self.handler.start_executor(executor);
    }

    pub fn local_storage(&self) -> Option<&LocalStorage> {
        self.handler.local_storage.get()
    }

    pub fn local_storage_mut(&mut self) -> Option<&mut LocalStorage> {
        self.handler.local_storage.get_mut()
    }

    pub fn enable(self) -> InterruptRef {
        let interrupt_ref = InterruptRef::new(self.handler);
        interrupt_ref.enable();
        interrupt_ref
    }
}

pub struct InterruptHandler<const PRIO: Priority, F: FnMut() + Send> {
    handler: ConstStaticCell<RawInterruptHandler>,
    closure: UnsafeCell<MaybeUninit<F>>,
}

impl<const PRIO: Priority, F: FnMut() + Send> InterruptHandler<PRIO, F> {
    pub const fn new(id: crate::pac::Interrupt) -> InterruptHandler<PRIO, F> {
        assert!(
            PRIO.is_interrupt(),
            "Interrupt handler priority must be an interrupt priority"
        );
        InterruptHandler {
            handler: ConstStaticCell::new(RawInterruptHandler::new(id as InterruptNumber, PRIO)),
            // SAFETY: locals may not be used before they are initialized in attach
            closure: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    pub fn init(&'static self) -> InterruptBuilder<PRIO, F> {
        let handler = self.handler.take();
        handler.suspendable = Suspendable::new_interrupt(handler);
        let closure = unsafe { &mut *self.closure.get() };
        InterruptBuilder { handler, closure }
    }

    unsafe extern "C" fn closure_wrapper(icb_ptr: *mut ::core::ffi::c_void) {
        let icb = unsafe { &*(icb_ptr as *const RawInterruptHandler) };
        let closure = unsafe { &mut *(icb.closure_ptr as *mut F) };

        // Call closure
        closure();

        // Poll executor
        icb.local_storage().map(|local_storage| {
            unsafe { local_storage.raw_get::<InterruptExecutor>() }
                .map(|executor| executor.poll(PollKind::OnInterrupt));
        });
    }
}

unsafe impl<const PRIO: Priority, F: FnMut() + Send> Sync for InterruptHandler<PRIO, F> {}

#[derive(Copy, Clone)]
pub struct InterruptRef(NonNull<RawInterruptHandler>);

impl InterruptRef {
    fn new(handler: &'static RawInterruptHandler) -> InterruptRef {
        InterruptRef(NonNull::from(handler))
    }

    unsafe fn from_ptr(ptr: *const RawInterruptHandler) -> InterruptRef {
        InterruptRef(unsafe { NonNull::new_unchecked(ptr as *mut RawInterruptHandler) })
    }

    pub fn base_priority(&self) -> Priority {
        unsafe { self.as_ref() }.base_priority
    }

    pub fn enable(&self) {
        InterruptLock::with(|key| {
            let handler = unsafe { self.as_ref() };
            if !handler.is_attached(key) {
                panic!("Attempt to enable interrupt that has not been attached");
            }
            handler.enable_interrupt(key);
        })
    }

    // TODO: disable interrupt

    pub fn set_pending_executor_poll(&self) {
        unsafe { self.as_ref() }.set_pending_executor_poll();
    }

    /// It is not in general safe to cast a pointer into a reference, and then
    /// dereference the reference. If you know that you are not violating
    /// any of the aliasing rules, you can use this method to obtain a reference
    /// to the underlying data and call re-entrant methods and read immutable data.
    pub(crate) unsafe fn as_ref(&self) -> &'static RawInterruptHandler {
        unsafe { self.0.as_ref() }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct InterruptVector {
    pub handler_ptr: *const (),
    pub icb_ptr: *const RawInterruptHandler,
}

static INTERRUPT_VECTORS: LockedCell<[InterruptVector; MAX_INTERRUPT_NUMBER + 1], InterruptLock> =
    LockedCell::new(
        [InterruptVector {
            handler_ptr: core::ptr::null(),
            icb_ptr: core::ptr::null(),
        }; MAX_INTERRUPT_NUMBER + 1],
    );

pub(crate) fn get_interrupt_vector(
    number: InterruptNumber,
    key: InterruptLockKey<'_>,
) -> InterruptVector {
    INTERRUPT_VECTORS.as_array_of_cells()[number as usize].get(key)
}

pub(crate) fn set_interrupt_vector(
    number: InterruptNumber,
    key: InterruptLockKey<'_>,
    vector: InterruptVector,
) {
    INTERRUPT_VECTORS.as_array_of_cells()[number as usize].set(key, vector)
}

#[unsafe(no_mangle)]
pub(crate) unsafe fn _private_kernel_interrupt_handler() {
    let claim = claim_interrupt();
    let interrupt_number = claim.get_interrupt_number();

    if interrupt_number > MAX_INTERRUPT_NUMBER as u16 {
        panic!("unexpected interrupt (IRQn={})", interrupt_number);
    }

    let cs = unsafe { InterruptLockKey::new() };

    let vector = get_interrupt_vector(interrupt_number as u16, cs);
    let handler_fn: fn(*const RawInterruptHandler) =
        unsafe { core::mem::transmute(vector.handler_ptr) };
    let icb_ptr = vector.icb_ptr as *mut _;

    unsafe {
        interrupt_context(icb_ptr, || {
            handler_fn(icb_ptr);
        });
    }

    complete_interrupt(claim);
}

#[inline(always)]
pub fn in_interrupt() -> bool {
    !CURRENT_INTERRUPT_CONTROL_BLOCK
        .load(Ordering::SeqCst)
        .is_null()
}

#[inline]
pub unsafe fn interrupt_context<R>(icb_ptr: *mut RawInterruptHandler, f: impl FnOnce() -> R) -> R {
    let prev_icb = switch_current_interrupt(icb_ptr);

    tracing::isr_enter();

    let rval = f();

    // If interrupt handler has woken up higher priority thread, a reschedule
    // to higher-priority thread is needed instead of returning to the current
    // thread. Wait until lowest priority nested interrupt returns.
    if prev_icb.is_null() {
        loop {
            // Execute pending interrupt executor polls. This must be done before
            // executing pending thread switches, as the executor polls may wake up
            // higher priority threads. Reschedule may wake up interrupt executors,
            // so poll them again until no more are pending.
            while let Some(pending_icb) = PENDING_INTERRUPT_EXECUTOR_POLLS.pop_front() {
                pending_icb.poll_executor();
            }

            tracing::isr_exit_to_scheduler();
            Scheduler::execute_pending_reschedule();

            // If reschedule woke up interrupt executors, poll again.
            if PENDING_INTERRUPT_EXECUTOR_POLLS.is_empty() {
                break;
            }
        }
    } else {
        tracing::isr_exit();
    }

    restore_current_interrupt(prev_icb);
    rval
}
