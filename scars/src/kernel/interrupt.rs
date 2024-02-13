use crate::cell::LockedCell;
use crate::kernel::list::LinkedList;
use crate::kernel::priority::{
    any_interrupt_priority, AnyPriority, AtomicPriorityPair, InterruptPriority, Priority,
    INVALID_PRIORITY,
};
use crate::kernel::scheduler::Scheduler;
use crate::kernel::task::{LockListTag, TaskControlBlock};
use crate::kernel::tracing;
use crate::sync::{
    ceiling_lock::RawCeilingLock,
    interrupt_lock::{InterruptLock, InterruptLockKey},
    KeyToken, Lock,
};
pub use critical_section::CriticalSection;
use scars_hal::{FlowController, InterruptController};

use crate::kernel::hal::{
    claim_interrupt, complete_interrupt, disable_interrupts, enable_interrupt, enable_interrupts,
    get_interrupt_priority, get_interrupt_threshold, set_interrupt_priority,
    set_interrupt_threshold, Context, MAX_INTERRUPT_NUMBER,
};
use crate::kernel::stack::StackCanary;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::{addr_of, addr_of_mut, NonNull};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

use super::scheduler::_private_current_task_context;

pub type InterruptNumber = u16;

#[macro_export]
macro_rules! make_interrupt_handler {
    ($intnum: expr, $prio : expr) => {{
        type T = impl ::core::marker::Sized + ::core::marker::Send + FnMut();
        static HANDLER: $crate::kernel::interrupt::InterruptHandler<{ $prio }, T> =
            $crate::kernel::interrupt::InterruptHandler::new($intnum);

        &HANDLER
    }};
}

extern "C" {
    static mut _isr_stack_start: u8;
}

static CURRENT_INTERRUPT_CONTROL_BLOCK: AtomicPtr<InterruptControlBlock> =
    AtomicPtr::new(core::ptr::null_mut());

pub(crate) fn switch_current_interrupt(
    icb_ptr: *mut InterruptControlBlock,
) -> *mut InterruptControlBlock {
    CURRENT_INTERRUPT_CONTROL_BLOCK.swap(icb_ptr, Ordering::SeqCst)
}

pub(crate) fn restore_current_interrupt(icb_ptr: *mut InterruptControlBlock) {
    CURRENT_INTERRUPT_CONTROL_BLOCK.store(icb_ptr, Ordering::SeqCst)
}

pub(crate) fn current_interrupt() -> Option<NonNull<InterruptControlBlock>> {
    NonNull::new(CURRENT_INTERRUPT_CONTROL_BLOCK.load(Ordering::SeqCst))
}

pub(crate) fn set_ceiling_threshold(ceiling: Priority) {
    if ceiling.is_valid() && ceiling.is_interrupt() {
        set_interrupt_threshold(ceiling.get_value());
    } else {
        set_interrupt_threshold(0);
    }
}

pub(crate) fn init_isr_stack_canary() {
    let canary = unsafe { &mut *(addr_of_mut!(_isr_stack_start) as *mut u8 as *mut StackCanary) };
    canary.init();
}

pub(crate) fn isr_stack_canary() -> &'static StackCanary {
    unsafe { &*(addr_of!(_isr_stack_start) as *const u8 as *const StackCanary) }
}

#[repr(align(16))]
#[repr(C)]
pub struct InterruptControlBlock {
    intnum: InterruptNumber,
    base_priority: Priority,

    closure_ptr: *const (),

    lock_priorities: AtomicPriorityPair,
    // The kernel must keep track of owned ceiling locks also in interrupt handlers,
    // because the locks might be released in any order.
    owned_locks: UnsafeCell<LinkedList<RawCeilingLock, LockListTag>>,
}

unsafe impl Sync for InterruptControlBlock {}

impl InterruptControlBlock {
    pub(crate) const fn new(
        intnum: InterruptNumber,
        prio: InterruptPriority,
    ) -> InterruptControlBlock {
        InterruptControlBlock {
            intnum,
            base_priority: Priority::interrupt_priority(prio),
            closure_ptr: core::ptr::null(),
            lock_priorities: AtomicPriorityPair::new((
                Priority::interrupt_priority(prio),
                INVALID_PRIORITY,
            )),
            owned_locks: UnsafeCell::new(LinkedList::new()),
        }
    }

    pub(crate) fn acquire_lock(&self, lock: &RawCeilingLock) {
        let locks = unsafe { &mut *self.owned_locks.get() };
        locks.insert_after_condition(lock, |list_lock, inserted_lock| {
            list_lock.ceiling_priority > inserted_lock.ceiling_priority
        });

        self.update_owned_lock_priority();
    }

    pub(crate) fn release_lock(&self, lock: &RawCeilingLock) {
        let locks = unsafe { &mut *self.owned_locks.get() };
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
        set_interrupt_vector(
            self.intnum,
            key,
            InterruptVector {
                handler_ptr,
                icb_ptr: self as *const _,
            },
        );
    }

    pub fn is_attached(&self, _key: InterruptLockKey<'_>) -> bool {
        !self.closure_ptr.is_null()
    }

    pub fn enable_interrupt(&self, _key: InterruptLockKey<'_>) {
        enable_interrupt(self.intnum);
    }

    // Returns previous priority
    pub(crate) fn raise_section_lock_priority(&self, new_priority: Priority) -> Priority {
        let new_priority = new_priority.max(self.base_priority);
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

    pub(crate) fn set_section_lock_priority(&self, new_priority: Priority) {
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
        let locks = unsafe { &mut *self.owned_locks.get() };
        let lock_priority = if let Some(head) = locks.head() {
            head.ceiling_priority
        } else {
            INVALID_PRIORITY
        };

        if let Ok((raised_priority, _)) = self.lock_priorities.fetch_update(
            Ordering::SeqCst,
            Ordering::SeqCst,
            |(raised_priority, _)| Some((raised_priority, lock_priority)),
        ) {
            let ceiling_priority = if lock_priority.is_valid() {
                Priority::max(raised_priority, lock_priority)
            } else {
                raised_priority
            };

            set_ceiling_threshold(ceiling_priority);
        }
    }

    /// Highest lock priority. Returns `INVALID_PRIORITY` if no locks owned by the interrupt.
    pub(crate) fn lock_priority<'key>(&self) -> Priority {
        let priorities = self.lock_priorities.load(Ordering::SeqCst);
        priorities.0.max(priorities.1)
    }

    pub(crate) fn active_priority(&self) -> Priority {
        let lock_priority = self.lock_priority();
        self.base_priority.max(lock_priority)
    }
}

pub struct InterruptHandler<const PRIO: InterruptPriority, F: FnMut() + Send> {
    handler: UnsafeCell<InterruptControlBlock>,
    closure: UnsafeCell<MaybeUninit<F>>,
}

impl<const PRIO: InterruptPriority, F: FnMut() + Send> InterruptHandler<PRIO, F> {
    pub const fn new(id: crate::pac::Interrupt) -> InterruptHandler<PRIO, F> {
        InterruptHandler {
            handler: UnsafeCell::new(InterruptControlBlock::new(id as InterruptNumber, PRIO)),
            // SAFETY: locals may not be used before they are initialized in attach
            closure: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    pub fn attach(&self, closure: F) {
        InterruptLock::with(|key| unsafe {
            let closure_ref = (&mut *self.closure.get()).write(closure);
            let closure_ptr = closure_ref as *const F as *const ();
            let handler = &mut *self.handler.get();
            handler.attach(Self::closure_wrapper as *const (), closure_ptr, key)
        });
    }

    unsafe extern "C" fn closure_wrapper(icb_ptr: *mut ::core::ffi::c_void) {
        let icb = &*(icb_ptr as *const InterruptControlBlock);
        let closure = &mut *(icb.closure_ptr as *mut F);
        closure();
    }

    pub fn enable_interrupt(&self) {
        InterruptLock::with(|key| {
            let handler = unsafe { &mut *self.handler.get() };
            if !handler.is_attached(key) {
                panic!("Attempt to enable interrupt that has not been attached");
            }
            handler.enable_interrupt(key);
        })
    }
}

unsafe impl<const PRIO: InterruptPriority, F: FnMut() + Send> Sync for InterruptHandler<PRIO, F> {}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct InterruptVector {
    pub handler_ptr: *const (),
    pub icb_ptr: *const InterruptControlBlock,
}

extern "C" {
    static __EXTERNAL_INTERRUPTS:
        LockedCell<[InterruptVector; MAX_INTERRUPT_NUMBER + 1], InterruptLock>;
}

pub(crate) fn get_interrupt_vector(
    number: InterruptNumber,
    key: InterruptLockKey<'_>,
) -> InterruptVector {
    unsafe { __EXTERNAL_INTERRUPTS.as_array_of_cells()[number as usize].get(key) }
}

pub(crate) fn set_interrupt_vector(
    number: InterruptNumber,
    key: InterruptLockKey<'_>,
    vector: InterruptVector,
) {
    unsafe { __EXTERNAL_INTERRUPTS.as_array_of_cells()[number as usize].set(key, vector) }
}

#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn DefaultHandler() {}

#[no_mangle]
pub(crate) unsafe fn _private_kernel_interrupt_handler() {
    let interrupt_number = claim_interrupt();

    if interrupt_number > MAX_INTERRUPT_NUMBER {
        panic!("unexpected interrupt (IRQn={})", interrupt_number);
    }

    let interrupt_prio = get_interrupt_priority(interrupt_number as u16);

    let cs = unsafe { InterruptLockKey::new() };

    let vector = get_interrupt_vector(interrupt_number as u16, cs);
    let handler_fn: fn(*const InterruptControlBlock) =
        unsafe { core::mem::transmute(vector.handler_ptr) };
    let icb_ptr = vector.icb_ptr as *mut _;

    interrupt_context(icb_ptr, |_cs| {
        // Enable interrupts up to given priority threshold for the duration of the interrupt
        // handler to allow nested higher priority interrupts.
        let old_prio = get_interrupt_threshold();
        set_interrupt_threshold(interrupt_prio);

        enable_interrupts();
        handler_fn(icb_ptr);
        disable_interrupts();
        // Interrupts are kept disabled until returning from the trap handler, in order to
        // avoid nesting interrupts of the same priority after interrupt is completed and
        // priority threshold restored. The critical section token 'cs' is again valid.

        // Restore interrupt threshold with interrupts disabled
        let _ = set_interrupt_threshold(old_prio);

        // Complete interrupt after interrupts have been disabled again so that completion
        // will not trigger new interrupt before the interrupt handler returns.
        complete_interrupt(interrupt_number as u16);
    });
}

#[inline(always)]
pub fn in_interrupt() -> bool {
    !CURRENT_INTERRUPT_CONTROL_BLOCK
        .load(Ordering::SeqCst)
        .is_null()
}

#[inline(never)]
#[no_mangle]
pub fn nested_isr_exit() {
    crate::printkln!("");
}

#[inline]
pub unsafe fn interrupt_context(
    icb_ptr: *mut InterruptControlBlock,
    f: impl FnOnce(InterruptLockKey),
) {
    let prev_icb = switch_current_interrupt(icb_ptr);

    let key = unsafe { InterruptLockKey::new() };

    tracing::isr_enter();

    f(key);

    // If interrupt handler has woken up higher priority task, a reschedule
    // to higher-priority task is needed instead of returning to the current
    // task. Wait until lowest priority nested interrupt returns.
    if prev_icb.is_null() {
        tracing::isr_exit_to_scheduler();
        Scheduler::execute_pending_task_switch(key);
    } else {
        nested_isr_exit();
        tracing::isr_exit();
    }

    restore_current_interrupt(prev_icb);
}

#[inline]
pub fn uncritical_section<R>(_ikey: InterruptLockKey<'_>, f: impl FnOnce() -> R) -> R {
    enable_interrupts();

    let rval = f();

    disable_interrupts();
    rval
}

#[no_mangle]
fn _save_additional_context() {}

#[no_mangle]
fn _restore_additional_context() {}
