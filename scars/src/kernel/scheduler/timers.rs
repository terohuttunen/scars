//! Kernel timer infrastructure.
//!
//! A timer is a deadline paired with an expiration callback. Armed
//! timers live in [`TimerQueue`], a deadline-ordered intrusive list
//! owned by the scheduler. When the hardware alarm fires, the scheduler
//! walks the head of the queue and dispatches each expired entry's
//! callback under the preempt lock; arming or repositioning a timer
//! reprograms the alarm to the new earliest deadline.
//!
//! The module is layered:
//!
//! - [`RawTimer`] is the type-erased queue node: a preempt-lock-protected
//!   deadline, an immutable expiration callback, and an
//!   atomically-published receiver pointer. All scheduler-side handling
//!   operates on `RawTimer`.
//!
//! - [`Timer<H>`] is a typed wrapper that binds the expiration callback
//!   to a [`TimerHandler`] impl. Dispatch is monomorphized over `H`,
//!   calling `H::on_expire` directly with a `Pin<&H>` receiver.
//!
//! - [`EventTimer`] is a concrete timer that sends a fixed [`Events`]
//!   set to an [`EventSender`] when its deadline elapses. Its `arm` and
//!   `cancel` methods are callable from any context, including
//!   interrupts: updates are staged under the interrupt lock and
//!   committed under the preempt lock, either inline or via the
//!   deferred-work queue when the preempt lock is unavailable.

use crate::cell::LockedCell;
use crate::events::{Events, sender::EventSender};
use crate::kernel::atomic_queue::{AtomicNode, AtomicQueue};
use crate::kernel::list::{LinkedList, LinkedListNode, LinkedListTag, Node, impl_linked};
use crate::kernel::scheduler::RawScheduler;
use crate::kernel::scheduler::work_queue::{
    PendingWorkEntry, PendingWorkHandler, RawPendingWorkEntry,
};
use crate::kernel::scheduler::{ExecStateTag, Scheduler};
use crate::kernel::waiter::{
    SUSPENDABLE_PENDING_RECONFIGURE, SUSPENDABLE_PENDING_RESUME, SUSPENDABLE_PENDING_SUSPEND,
    SUSPENDABLE_PENDING_WAKEUP, WaitQueueEntry,
};
use crate::priority::Priority;
use crate::sync::interrupt_lock::InterruptLock;
use crate::sync::preempt_lock::{PreemptLock, PreemptLockKey};
use crate::time::Instant;
use core::cell::Cell;
use core::marker::PhantomData;
use core::pin::Pin;
use core::sync::atomic::{AtomicPtr, Ordering};

/// Marker tag for the intrusive list of armed [`RawTimer`]s.
pub struct TimerQueueTag {}

impl LinkedListTag for TimerQueueTag {}

/// Deadline-ordered list of armed timers, owned by the scheduler.
pub type TimerQueue = LinkedList<RawTimer, TimerQueueTag>;

/// Type-erased expiration callback signature stored in [`RawTimer`].
/// The first argument is the receiver pointer published via
/// [`RawTimer::set_receiver`].
pub type RawTimerExpireCallback = fn(*const (), Pin<&mut RawScheduler>, PreemptLockKey<'_>);

/// Kernel-internal timer node.
///
/// `on_expire` is fixed at construction. `deadline` is mutable under
/// the preempt lock. `receiver` is published lock-free via an atomic
/// store. The node is in [`TimerQueue`] iff `deadline` is `Some`.
pub struct RawTimer {
    deadline: LockedCell<Option<Instant>, PreemptLock>,
    on_expire: RawTimerExpireCallback,
    receiver: AtomicPtr<()>,
    node: Node<Self, TimerQueueTag>,
}

impl RawTimer {
    /// Construct a disarmed timer that dispatches through `on_expire`.
    /// `const` for placement in static storage.
    pub const fn with_callback(on_expire: RawTimerExpireCallback) -> Self {
        Self {
            deadline: LockedCell::new(None),
            on_expire,
            receiver: AtomicPtr::new(core::ptr::null_mut()),
            node: Node::new(),
        }
    }

    pub fn get_deadline(&self, pkey: PreemptLockKey<'_>) -> Option<Instant> {
        self.deadline.get(pkey)
    }

    pub fn is_armed(&self) -> bool {
        self.node.in_list()
    }

    /// Publish the receiver pointer with [`Ordering::Release`].
    /// Idempotent and lock-free.
    pub fn set_receiver(&self, receiver: *const ()) {
        self.receiver.store(receiver as *mut (), Ordering::Release);
    }

    /// Write `deadline` under the preempt lock and reposition the node
    /// in [`TimerQueue`]. Reprograms the hardware alarm. `None` removes
    /// the node from the queue.
    pub fn set_deadline(
        self: Pin<&Self>,
        pkey: PreemptLockKey<'_>,
        mut scheduler: Pin<&mut RawScheduler>,
        deadline: Option<Instant>,
    ) {
        self.deadline.set(pkey, deadline);
        scheduler.as_mut().timer_reconfigured(pkey, self);
    }

    /// Dispatch through `on_expire` with the currently-published
    /// receiver, loaded with [`Ordering::Acquire`]. A `null` receiver
    /// returns without dispatching.
    ///
    /// # Safety
    ///
    /// The most recently published receiver must point to a live,
    /// pinned instance of the receiver type expected by `on_expire`.
    pub unsafe fn expire(&self, scheduler: Pin<&mut RawScheduler>, pkey: PreemptLockKey<'_>) {
        let receiver = self.receiver.load(Ordering::Acquire);
        if !receiver.is_null() {
            (self.on_expire)(receiver as *const (), scheduler, pkey);
        }
    }
}

impl_linked!(node, RawTimer, TimerQueueTag);

/// Compile-time binding of a receiver type `Self` to a [`Timer<Self>`]'s
/// expiration callback.
///
/// `on_expire` receives `Pin<&'static Self>`: armed timers are queue
/// nodes that the scheduler may dispatch arbitrarily later than `arm`
/// returns, so the receiver must outlive any arm.
pub trait TimerHandler: Sized + 'static {
    fn on_expire(this: Pin<&'static Self>, sched: Pin<&mut RawScheduler>, pkey: PreemptLockKey<'_>);
}

/// Typed wrapper over [`RawTimer`] parameterized by a [`TimerHandler`]
/// impl.
///
/// # Example
///
/// A periodic blink timer that re-arms itself on each expiration:
///
/// ```ignore
/// struct Blink {
///     timer: Timer<Self>,
///     period: Duration,
/// }
///
/// impl Blink {
///     fn timer_pin(self: Pin<&Self>) -> Pin<&Timer<Self>> {
///         unsafe { self.map_unchecked(|b| &b.timer) }
///     }
/// }
///
/// impl TimerHandler for Blink {
///     fn on_expire(
///         this: Pin<&'static Self>,
///         sched: Pin<&mut RawScheduler>,
///         pkey: PreemptLockKey<'_>,
///     ) {
///         toggle_led();
///         let next = Instant::now() + this.period;
///         this.timer_pin().arm(pkey, sched, this, Some(next));
///     }
/// }
///
/// // From a context that holds the preempt lock, with `blink: Pin<&'static Blink>`:
/// blink.timer_pin().arm(pkey, sched, blink, Some(first_deadline));
/// ```
#[repr(transparent)]
pub struct Timer<H: TimerHandler> {
    raw: RawTimer,
    _phantom: PhantomData<fn() -> H>,
}

/// Monomorphized adapter from [`RawTimerExpireCallback`] to
/// [`TimerHandler::on_expire`].
fn timer_trampoline<H: TimerHandler>(
    receiver: *const (),
    sched: Pin<&mut RawScheduler>,
    pkey: PreemptLockKey<'_>,
) {
    // SAFETY: `receiver` was published by `Timer::<H>::arm`, which only
    // accepts `Pin<&'static H>`.
    let this: Pin<&'static H> = unsafe { Pin::new_unchecked(&*(receiver as *const H)) };
    H::on_expire(this, sched, pkey);
}

impl<H: TimerHandler> Timer<H> {
    /// Construct a disarmed `Timer<H>`.
    pub const fn new() -> Self {
        Self {
            raw: RawTimer::with_callback(timer_trampoline::<H>),
            _phantom: PhantomData,
        }
    }

    #[allow(dead_code)]
    pub fn get_deadline(&self, pkey: PreemptLockKey<'_>) -> Option<Instant> {
        self.raw.get_deadline(pkey)
    }

    pub fn is_armed(&self) -> bool {
        self.raw.is_armed()
    }

    /// Publish `owner` as the receiver and write `deadline`. `None`
    /// disarms. The receiver pointer is rewritten on every call.
    pub fn arm(
        self: Pin<&Self>,
        pkey: PreemptLockKey<'_>,
        sched: Pin<&mut RawScheduler>,
        receiver: Pin<&'static H>,
        deadline: Option<Instant>,
    ) {
        self.raw_pin()
            .set_receiver(receiver.get_ref() as *const _ as *const ());
        self.raw_pin().set_deadline(pkey, sched, deadline);
    }

    /// Pin projection to the inner [`RawTimer`].
    pub fn raw_pin(self: Pin<&Self>) -> Pin<&RawTimer> {
        unsafe { self.map_unchecked(|t| &t.raw) }
    }
}

/// Snapshot of an [`EventTimer`]'s armed configuration. Held in a
/// single cell so deadline and target update atomically. `None` means
/// disarmed.
#[derive(Copy, Clone)]
struct EventTimerConfig {
    deadline: Instant,
    sender: EventSender,
    events: Events,
}

/// Timer that delivers a fixed set of [`Events`] to an [`EventSender`]
/// when its deadline elapses.
///
/// `arm` and `cancel` may be called from any context. Changes are
/// staged in `pending` under the interrupt lock and applied to
/// `current` and the kernel timer atomically under the preempt lock,
/// either inline or via the deferred-work queue.
pub struct EventTimer {
    timer: Timer<Self>,
    /// Live config read by [`TimerHandler::on_expire`]. Updated under
    /// the preempt lock in lockstep with the kernel timer's deadline.
    current: LockedCell<Option<EventTimerConfig>, PreemptLock>,
    /// Staged config awaiting application under the preempt lock.
    pending: LockedCell<Option<EventTimerConfig>, InterruptLock>,
    /// Work entry that requests deferred application when the preempt
    /// lock cannot be acquired inline.
    pending_work: PendingWorkEntry<Self>,
}

impl EventTimer {
    pub const fn new() -> Self {
        Self {
            timer: Timer::new(),
            current: LockedCell::new(None),
            pending: LockedCell::new(None),
            pending_work: PendingWorkEntry::new(),
        }
    }

    fn get_timer(self: Pin<&Self>) -> Pin<&Timer<EventTimer>> {
        unsafe { self.map_unchecked(|t| &t.timer) }
    }

    pub(crate) fn get_pending_work(self: Pin<&Self>) -> Pin<&RawPendingWorkEntry> {
        let typed: Pin<&PendingWorkEntry<EventTimer>> =
            unsafe { self.map_unchecked(|t| &t.pending_work) };
        typed.raw()
    }

    pub fn is_armed(&self) -> bool {
        self.timer.is_armed()
    }

    #[allow(dead_code)]
    pub(crate) fn get_deadline(&self, pkey: PreemptLockKey<'_>) -> Option<Instant> {
        self.timer.get_deadline(pkey)
    }

    /// Schedule `events` to be sent to `sender` at `deadline`. Replaces
    /// any prior arming.
    pub fn arm(self: Pin<&'static Self>, deadline: Instant, sender: EventSender, events: Events) {
        self.set_pending(Some(EventTimerConfig {
            deadline,
            sender,
            events,
        }));
    }

    /// Disarm the timer.
    pub fn cancel(self: Pin<&'static Self>) {
        self.set_pending(None);
    }

    /// Stage `config` in `pending` under the interrupt lock. If the
    /// preempt lock is acquirable, apply via [`reconfigure`]; otherwise
    /// queue [`pending_work`] for deferred application by the
    /// scheduler.
    fn set_pending(self: Pin<&'static Self>, config: Option<EventTimerConfig>) {
        InterruptLock::with(|ikey| self.pending.set(ikey, config));
        if PreemptLock::try_with(|pkey| {
            let mut scheduler = Scheduler::pin_instance().borrow_mut(pkey);
            self.reconfigure(pkey, scheduler.as_mut());
        })
        .is_err()
        {
            // Required before queueing: a pending_work entry with a null
            // receiver is skipped on dispatch.
            self.pending_work.set_receiver(self);
            Scheduler::instance().deferred_work_queue.queue_work(
                self.get_pending_work(),
                SUSPENDABLE_PENDING_RECONFIGURE,
                None,
            );
        }
    }

    /// Read the staged `pending` config under the interrupt lock,
    /// mirror it to `current`, and update the kernel timer's deadline.
    /// All under the preempt lock so a concurrent firing observes the
    /// matching `(deadline, target)` pair.
    fn reconfigure(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'_>,
        scheduler: Pin<&mut RawScheduler>,
    ) {
        let config = InterruptLock::with(|ikey| self.pending.get(ikey));
        self.current.set(pkey, config);
        self.get_timer()
            .arm(pkey, scheduler, self, config.map(|c| c.deadline));
    }
}

impl PendingWorkHandler for EventTimer {
    fn complete(
        this: Pin<&'static Self>,
        pkey: PreemptLockKey<'_>,
        scheduler: Pin<&mut RawScheduler>,
        work: u32,
    ) {
        if work & SUSPENDABLE_PENDING_RECONFIGURE == 0 {
            return;
        }
        this.reconfigure(pkey, scheduler);
    }
}

impl TimerHandler for EventTimer {
    fn on_expire(
        this: Pin<&'static Self>,
        _sched: Pin<&mut RawScheduler>,
        pkey: PreemptLockKey<'_>,
    ) {
        if let Some(config) = this.current.get(pkey) {
            config.sender.send_events(config.events);
        }
    }
}

// Internal types contain `Cell` and `*const R` raw pointers that are
// not auto-`Sync`. All field access is gated by a kernel lock.
unsafe impl Sync for EventTimer {}
unsafe impl Send for EventTimer {}

impl RawScheduler {
    /// Reposition `timer` in [`TimerQueue`] for its currently-set
    /// deadline and reprogram the hardware alarm. The caller must have
    /// already written the deadline.
    pub(crate) fn timer_reconfigured(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        timer: Pin<&RawTimer>,
    ) {
        let deadline = timer.get_deadline(pkey);
        if timer.is_armed() {
            self.as_mut().timer_queue_mut().remove(timer);
        }
        if deadline.is_some() {
            self.as_mut()
                .timer_queue_mut()
                .insert_after(timer, |queue_timer| {
                    queue_timer.get_deadline(pkey) <= deadline
                });
        }
        self.as_ref().reprogram_alarm(pkey);
    }

    pub(crate) fn wakeup_timer(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        timer: Pin<&RawTimer>,
    ) {
        unsafe { timer.expire(self.as_mut(), pkey) };
        // Note: does not check for need to reschedule, as this is called from reschedule.
    }
}
