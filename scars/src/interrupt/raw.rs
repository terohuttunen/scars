//! Core interrupt handler implementation
//!
//! This module contains the RawInterruptHandler which represents the core hardware
//! interrupt handling infrastructure including priority management, lock handling,
//! and event configuration.

use super::{InterruptNumber, vector::InterruptVector};
use crate::events::raw::RawEventHandler;
use crate::kernel::hal::CoreId;
#[cfg(feature = "raii-locks")]
use crate::kernel::list::LinkedList;
use crate::local::LocalStorage;
use crate::priority::{AtomicPriority, AtomicPriorityOpt, Priority, PriorityOpt};
#[cfg(feature = "raii-locks")]
use crate::sync::lock::ceiling_lock::RawCeilingLock;
use crate::sync::lock::interrupt_lock::CoreInterruptLockKey;
#[cfg(feature = "raii-locks")]
use crate::thread::LockListTag;

use crate::sync::atomic::Ordering;
use core::cell::Cell;
#[cfg(feature = "raii-locks")]
use core::cell::UnsafeCell;
#[cfg(feature = "raii-locks")]
use core::pin::Pin;

/// Core interrupt handler structure
#[repr(align(16))]
#[repr(C)]
pub(crate) struct RawInterruptHandler {
    intnum: InterruptNumber,
    base_priority: Priority,

    /// Core this handler is bound to. Set at construction from the
    /// wrapping `InterruptHandler<PRIO, F, CORE>`.
    pub core: CoreId,

    // Nesting ceiling lock priority
    pub(crate) nesting_lock_priority: AtomicPriorityOpt,

    // Guarded (scoped) ceiling Lock priority
    #[cfg(feature = "raii-locks")]
    pub(crate) lock_priority: AtomicPriorityOpt,

    // Effective priority of the thread. This is the maximum of the base priority and the
    // priority of any lock held by the thread.
    pub(crate) priority: AtomicPriority,

    // The kernel must keep track of owned ceiling locks also in interrupt handlers,
    // because the locks might be released in any order.
    #[cfg(feature = "raii-locks")]
    owned_locks: UnsafeCell<LinkedList<RawCeilingLock, LockListTag>>,

    closure_ptr: *const (),

    /// Local storage. Owns its own list, or may be configured via
    /// `with_shared_storage` to redirect to another owner's list.
    pub(crate) local_storage: LocalStorage,

    /// Pointer to the event handler currently executing on this
    /// interrupt (set/cleared by `events::context::event_handler_context`).
    /// Null when no event handler is running. Local storage dispatch on
    /// this interrupt picks the event handler's `LocalStorage` when set,
    /// otherwise this interrupt's own `local_storage`.
    pub(crate) current_event_handler: Cell<*const RawEventHandler>,
}

#[allow(dead_code)]
impl RawInterruptHandler {
    pub const fn new(prio: Priority, core: CoreId) -> RawInterruptHandler {
        RawInterruptHandler {
            intnum: 0,
            base_priority: prio,
            core,
            closure_ptr: core::ptr::null(),
            nesting_lock_priority: AtomicPriorityOpt::new(PriorityOpt::none()),
            #[cfg(feature = "raii-locks")]
            lock_priority: AtomicPriorityOpt::new(PriorityOpt::none()),
            priority: AtomicPriority::new(prio),
            #[cfg(feature = "raii-locks")]
            owned_locks: UnsafeCell::new(LinkedList::new()),
            local_storage: LocalStorage::new(),
            current_event_handler: Cell::new(core::ptr::null()),
        }
    }

    pub fn init_at(this: *mut Self, intnum: InterruptNumber) {
        unsafe {
            (*this).intnum = intnum;
        }
    }

    pub fn interrupt_number(&self) -> InterruptNumber {
        self.intnum
    }

    pub fn base_priority(&self) -> Priority {
        self.base_priority
    }

    pub fn effective_priority(&self) -> Priority {
        self.priority.load(Ordering::Acquire)
    }

    pub fn set_effective_priority(&self, prio: Priority) {
        self.priority.store(prio, Ordering::Release);
    }

    pub fn closure_ptr(&self) -> *const () {
        self.closure_ptr
    }

    pub fn set_closure_ptr(&mut self, ptr: *const ()) {
        self.closure_ptr = ptr;
    }

    /// `LocalStorage` to use for storage dispatch in this interrupt context.
    ///
    /// If an event handler is currently executing (set by
    /// `events::context::event_handler_context`), returns the event
    /// handler's `LocalStorage`. Otherwise returns this interrupt's own.
    pub fn current_local_storage(&'static self) -> &'static LocalStorage {
        let eh = self.current_event_handler.get();
        if eh.is_null() {
            &self.local_storage
        } else {
            // SAFETY: pointer was set by `event_handler_context` from a
            // `*mut RawEventHandler` referring to a 'static event handler.
            let eh: &'static RawEventHandler = unsafe { &*eh };
            eh.local_storage()
        }
    }

    // Priority and lock management methods
    pub fn nesting_lock_priority(&self) -> PriorityOpt {
        self.nesting_lock_priority.load(Ordering::Acquire)
    }

    pub fn set_nesting_lock_priority(&self, prio: PriorityOpt) {
        self.nesting_lock_priority.store(prio, Ordering::Release);
    }

    #[cfg(feature = "raii-locks")]
    pub fn lock_priority(&self) -> PriorityOpt {
        self.lock_priority.load(Ordering::Acquire)
    }

    #[cfg(feature = "raii-locks")]
    pub fn set_lock_priority(&self, prio: PriorityOpt) {
        self.lock_priority.store(prio, Ordering::Release);
    }

    /// Get owned locks list (unsafe - caller must ensure proper synchronization)
    #[cfg(feature = "raii-locks")]
    #[allow(dead_code)]
    pub(crate) unsafe fn owned_locks(&self) -> &LinkedList<RawCeilingLock, LockListTag> {
        unsafe { &*self.owned_locks.get() }
    }

    /// Get mutable owned locks list (unsafe - caller must ensure proper synchronization)
    #[cfg(feature = "raii-locks")]
    #[allow(dead_code)]
    pub(crate) unsafe fn owned_locks_mut(&self) -> &mut LinkedList<RawCeilingLock, LockListTag> {
        unsafe { &mut *self.owned_locks.get() }
    }

    /// Enable this interrupt at the hardware level. The lock key proves
    /// the caller holds the interrupt lock; `debug_assert` checks that
    /// it's the lock for this handler's core (NVIC writes are
    /// local-core only).
    pub fn enable_interrupt(&self, key: crate::sync::lock::interrupt_lock::InterruptLockKey<'_>) {
        debug_assert_eq!(key.core, self.core);
        let _ = key;
        crate::kernel::hal::enable_interrupt(self.intnum);
    }

    /// Disable this interrupt at the hardware level. Same `key`-core
    /// constraint as [`enable_interrupt`].
    pub fn disable_interrupt(&self, key: crate::sync::lock::interrupt_lock::InterruptLockKey<'_>) {
        debug_assert_eq!(key.core, self.core);
        let _ = key;
        crate::kernel::hal::disable_interrupt(self.intnum);
    }

    /// Get a pointer to this interrupt handler
    pub fn as_ptr(&self) -> *const RawInterruptHandler {
        self as *const RawInterruptHandler
    }

    /// Update effective priority based on base, lock, and nesting lock priorities
    fn update_priority(&self) {
        let nesting_lock_priority = self.nesting_lock_priority.load(Ordering::SeqCst);

        let new_priority = self.base_priority.max_valid(nesting_lock_priority);
        #[cfg(feature = "raii-locks")]
        let new_priority = new_priority.max_valid(self.lock_priority.load(Ordering::SeqCst));

        self.priority.store(new_priority, Ordering::SeqCst);
    }

    /// Acquire a ceiling lock (unsafe - caller must ensure proper synchronization)
    #[cfg(feature = "raii-locks")]
    pub unsafe fn ceiling_lock_acquired(self: Pin<&Self>, lock: Pin<&RawCeilingLock>) {
        let mut locks = unsafe { Pin::new_unchecked(&mut *(self.owned_locks.get())) };
        locks.as_mut().insert_after(lock, |list_lock| {
            list_lock.ceiling_priority > lock.ceiling_priority
        });

        let lock_priority = locks
            .as_ref()
            .head()
            .map_or_else(PriorityOpt::none, |head| {
                PriorityOpt::from(head.ceiling_priority)
            });

        self.lock_priority.store(lock_priority, Ordering::SeqCst);
        self.update_priority();
    }

    /// Release a ceiling lock (unsafe - caller must ensure lock was acquired)
    #[cfg(feature = "raii-locks")]
    pub unsafe fn ceiling_lock_released(&self, lock: Pin<&RawCeilingLock>) {
        let mut locks = unsafe { Pin::new_unchecked(&mut *(self.owned_locks.get())) };
        locks.as_mut().remove(lock);

        let lock_priority = locks
            .as_ref()
            .head()
            .map_or_else(PriorityOpt::none, |head| {
                PriorityOpt::from(head.ceiling_priority)
            });

        self.lock_priority.store(lock_priority, Ordering::SeqCst);
        self.update_priority();
    }

    /// Raise nesting lock priority (returns previous priority)
    pub fn raise_nesting_lock_priority(&self, new_priority: Priority) -> PriorityOpt {
        let old_priority = self
            .nesting_lock_priority
            .swap(new_priority.into(), Ordering::SeqCst);

        if old_priority > new_priority.into() {
            crate::runtime_error!(crate::kernel::exception::RuntimeError::CeilingPriorityViolation);
        }

        self.update_priority();
        old_priority
    }

    /// Get current effective priority (method version for compatibility)
    pub fn priority(&self) -> Priority {
        self.priority.load(Ordering::SeqCst)
    }

    /// Attach event handler
    pub unsafe fn attach<const CORE: CoreId>(
        &mut self,
        handler_ptr: *const (),
        closure_ptr: *const (),
        key: CoreInterruptLockKey<'_, CORE>,
    ) {
        use super::set_interrupt_vector;
        use crate::kernel::hal::set_interrupt_priority;

        self.closure_ptr = closure_ptr;
        let _ = set_interrupt_priority(self.intnum, self.base_priority.get_value());

        set_interrupt_vector(
            self.intnum,
            key.erase(),
            InterruptVector::new(handler_ptr, self as *const _),
        );
    }

    pub fn is_attached(
        &self,
        key: crate::sync::lock::interrupt_lock::InterruptLockKey<'_>,
    ) -> bool {
        debug_assert_eq!(key.core, self.core);
        !super::get_interrupt_vector(self.intnum, key)
            .handler_ptr
            .is_null()
    }
}

unsafe impl Sync for RawInterruptHandler {}
unsafe impl Send for RawInterruptHandler {}
