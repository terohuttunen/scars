//! Wait queue with notify-one / notify-all wake semantics.
//!
//! Waiters call [`Notify::arm`] from inside a
//! [`Protected::with_barrier`] / [`Protected::with_barrier_until`]
//! closure that shares the same `L`. Notifiers call
//! [`Notify::notify_one`] or [`Notify::notify_all`]. The
//! highest-priority waiter is woken first.
//!
//! For wait paths with no paired user data, [`Notify::wait`],
//! [`Notify::wait_until`], and [`Notify::async_wait`] block directly
//! on the notify.
//!
//! ```ignore
//! use scars::prelude::*;
//! use scars::sync::{CoreCeilingLock, Notify, Protected, BarrierResult};
//!
//! const CEILING: Priority = Priority::thread(5);
//!
//! static DATA: Protected<u32, CoreCeilingLock<CEILING>> = Protected::new(0);
//! static READY: Notify<CoreCeilingLock<CEILING>> = Notify::new();
//!
//! // Waiter: block until DATA > 0, then consume one.
//! DATA.with_barrier(|key, d| {
//!     if *d > 0 {
//!         *d -= 1;
//!         BarrierResult::Done(())
//!     } else {
//!         BarrierResult::Wait(READY.arm(key))
//!     }
//! });
//!
//! // Notifier: increment and wake a waiter.
//! DATA.with(|_, d| {
//!     *d += 1;
//!     READY.notify_one();
//! });
//! ```

use crate::kernel::scheduler::{ExecutionContext, RESCHEDULE_KIND_BLOCK_CURRENT, Scheduler};
use crate::kernel::waiter::{WaitList, WaitQueueEntry, WaitQueueHandle, WaitQueueVTable};
use crate::runtime_error;
use crate::sync::lock::preempt_lock::PreemptLockKey;
use crate::sync::protected::WaitMarker;
use crate::sync::{NestingLock, PreemptLock, Protected, TimedOut};
use crate::task::raw_task::RawTask;
use crate::time::Instant;
use core::future::poll_fn;
use core::pin::Pin;

/// Wait queue with notify-one / notify-all wake semantics. See the
/// [module docs](self) for the integration patterns.
pub struct Notify<L: NestingLock> {
    wait_list: Protected<WaitList, L>,
}

impl<L: NestingLock> Notify<L> {
    /// Create an empty notify.
    pub const fn new() -> Notify<L> {
        Notify {
            wait_list: Protected::new(WaitList::new()),
        }
    }

    /// Pin-project `&self.wait_list`. Sound because `Notify` lives in
    /// a static (immovable) by construction.
    #[inline]
    fn wait_list_pinned(&self) -> Pin<&Protected<WaitList, L>> {
        // SAFETY: see fn comment.
        unsafe { Pin::new_unchecked(&self.wait_list) }
    }

    /// Push the thread's wait entry into the wait list and write its
    /// `wait_queue` handle. Caller commits the suspend.
    fn arm_current_thread(&self, key: L::Key<'_>, thread: Pin<&'static crate::thread::RawThread>) {
        self.wait_list_pinned()
            .with_pin_key(key, |_, list: Pin<&mut WaitList>| {
                PreemptLock::with(|pkey| {
                    let entry = thread.get_wait_entry();
                    list.push_priority(pkey, entry);
                    let handle = unsafe {
                        WaitQueueHandle::from_raw(
                            &self.wait_list as *const Protected<WaitList, L> as *const (),
                            Self::VTABLE as *const _,
                        )
                    };
                    thread.arm_wait(pkey, handle);
                });
            });
    }

    /// Enqueue the current thread on this notify. The returned
    /// [`WaitMarker`] must be bubbled out of the enclosing
    /// [`Protected::with_barrier`] / [`Protected::with_barrier_until`]
    /// closure as `BarrierResult::Wait(marker)`; the barrier loop
    /// suspends the thread on commit. The thread wakes on
    /// [`notify_one`](Self::notify_one) or
    /// [`notify_all`](Self::notify_all).
    ///
    /// Must be called from a thread context. Raises
    /// [`RuntimeError::InterruptHandlerViolation`] in an interrupt
    /// handler.
    pub fn arm(&'static self, key: L::Key<'_>) -> WaitMarker {
        let thread = match Scheduler::current_execution_context() {
            ExecutionContext::Thread(t) => t,
            ExecutionContext::Interrupt(_) => {
                runtime_error!(RuntimeError::InterruptHandlerViolation);
            }
        };
        self.arm_current_thread(key, thread);
        WaitMarker::new()
    }

    /// Arm the current thread on this notify without committing the
    /// block. Used by `wait`/`wait_until` and by composite primitives
    /// (the inheritance lock) that must arm inside another lock's
    /// closure and commit the block separately.
    ///
    /// Must be called from a thread context. Raises
    /// [`RuntimeError::InterruptHandlerViolation`] in an interrupt
    /// handler.
    pub(crate) fn arm_current(&self) {
        let thread = match Scheduler::current_execution_context() {
            ExecutionContext::Thread(t) => t,
            ExecutionContext::Interrupt(_) => {
                runtime_error!(RuntimeError::InterruptHandlerViolation);
            }
        };
        L::with(|key| self.arm_current_thread(key, thread));
    }

    /// Arm the current thread, run `between`, then suspend until
    /// [`notify_one`](Self::notify_one) / [`notify_all`](Self::notify_all)
    /// wakes it.
    ///
    /// `between` runs *after* the waiter is queued and *before* the
    /// block is committed. A condvar passes `|| guard.unlock()` here:
    /// releasing the mutex only after arming closes the lost-wakeup
    /// window, and a notify delivered in the gap between `between` and
    /// the commit is absorbed by the drain gate (`block_current`).
    /// Committing the block outside the wait-list `L::with` also lets
    /// the lock's ceiling threshold drop before the drain runs.
    ///
    /// Must be called from a thread context. Raises
    /// [`RuntimeError::InterruptHandlerViolation`] in an interrupt
    /// handler.
    pub fn wait_with(&self, between: impl FnOnce()) {
        self.arm_current();
        between();
        Scheduler::set_pending_reschedule(RESCHEDULE_KIND_BLOCK_CURRENT);
    }

    /// Like [`wait_with`](Self::wait_with), but bounded by `deadline`.
    /// Returns `Ok(())` on notifier wake or `Err(TimedOut)` if the
    /// deadline elapses first.
    pub fn wait_until_with(
        &self,
        deadline: Instant,
        between: impl FnOnce(),
    ) -> Result<(), TimedOut> {
        self.arm_current();
        between();
        Scheduler::set_current_pending_block_deadline(Some(deadline));
        Scheduler::set_pending_reschedule(RESCHEDULE_KIND_BLOCK_CURRENT);
        Scheduler::take_last_wait_timed_out()
    }

    /// Block the current thread until [`notify_one`](Self::notify_one)
    /// or [`notify_all`](Self::notify_all) wakes it. For callers that
    /// pair this notify with user data, use [`arm`](Self::arm) inside
    /// [`Protected::with_barrier`] instead.
    ///
    /// Must be called from a thread context. Raises
    /// [`RuntimeError::InterruptHandlerViolation`] in an interrupt
    /// handler.
    pub fn wait(&self) {
        self.wait_with(|| {});
    }

    /// Like [`wait`](Self::wait), but bounded by `deadline`. Returns
    /// `Ok(())` on notifier wake or `Err(TimedOut)` if the deadline
    /// elapses first.
    pub fn wait_until(&self, deadline: Instant) -> Result<(), TimedOut> {
        self.wait_until_with(deadline, || {})
    }

    /// Park the current async task until
    /// [`notify_one`](Self::notify_one) or
    /// [`notify_all`](Self::notify_all) wakes it. The async
    /// counterpart of [`wait`](Self::wait).
    pub async fn async_wait(&'static self) {
        let mut waiter_queued: bool = false;
        poll_fn(|cx| {
            self.wait_list_pinned()
                .with_pin(|_, mut list: Pin<&mut WaitList>| {
                    let task = unsafe { &*(cx.waker().data() as *const RawTask) };
                    if !waiter_queued {
                        // SAFETY: `task.waiter` is pinned in the task,
                        // which itself is pinned for its lifetime.
                        let entry = unsafe { Pin::new_unchecked(&task.waiter) };
                        PreemptLock::with(|pkey| {
                            list.as_mut().push_priority(pkey, entry);
                        });
                        waiter_queued = true;
                        core::task::Poll::Pending
                    } else if !task.waiter.wait_queue_link.in_list() {
                        core::task::Poll::Ready(())
                    } else {
                        core::task::Poll::Pending
                    }
                })
        })
        .await
    }

    /// Wake the highest-priority waiter, if any. No-op if no waiter
    /// is enqueued.
    pub fn notify_one(&self) {
        self.wait_list_pinned().with_pin(|_, mut list| {
            if let Some(entry) = list.as_mut().pop_front() {
                entry.notify();
            }
        });
    }

    /// Wake every waiter currently enqueued.
    pub fn notify_all(&self) {
        self.wait_list_pinned().with_pin(|_, mut list| {
            while let Some(entry) = list.as_mut().pop_front() {
                entry.notify();
            }
        });
    }

    const VTABLE: &'static WaitQueueVTable = &WaitQueueVTable {
        try_remove: Self::vt_try_remove,
        try_reinsert: Self::vt_try_reinsert,
    };

    unsafe fn vt_try_remove(
        queue: *const (),
        _pkey: PreemptLockKey<'_>,
        entry: *const WaitQueueEntry,
    ) -> Result<bool, ()> {
        let entry = unsafe { Pin::new_unchecked(&*entry) };
        // Fast path: an unlinked entry was already popped by a
        // notifier; nothing to remove. The read is stable: the entry
        // is re-linked only by its own thread arming a new wait,
        // which cannot run while this core's preempt lock is held.
        // (The notifier calling `try_resume_thread` from inside its
        // own `notify_one` closure relies on this path — the wait
        // list is in use at that point.)
        if !entry.wait_queue_link.in_list() {
            return Ok(false);
        }
        // SAFETY: vtable is invoked only while the owning `Notify` is
        // alive; the queue pointer is therefore valid. Notify is
        // pinned-by-construction, so the pin lift is sound.
        let wait_list = unsafe { Pin::new_unchecked(&*(queue as *const Protected<WaitList, L>)) };
        wait_list
            .kernel_try_with_pin(|_, mut list: Pin<&mut WaitList>| {
                // Re-check under `L`: a notifier may have popped the
                // entry between the fast-path read and this
                // acquisition.
                if entry.wait_queue_link.in_list() {
                    list.as_mut().remove(entry);
                    true
                } else {
                    false
                }
            })
            .map_err(|_| ())
    }

    unsafe fn vt_try_reinsert(
        queue: *const (),
        pkey: PreemptLockKey<'_>,
        entry: *const WaitQueueEntry,
    ) -> Result<(), ()> {
        let entry = unsafe { Pin::new_unchecked(&*entry) };
        // Fast path: as in `vt_try_remove`.
        if !entry.wait_queue_link.in_list() {
            return Ok(());
        }
        // SAFETY: as in `vt_try_remove`.
        let wait_list = unsafe { Pin::new_unchecked(&*(queue as *const Protected<WaitList, L>)) };
        wait_list
            .kernel_try_with_pin(|_, list: Pin<&mut WaitList>| {
                // Re-check under `L`: a concurrent pop may have
                // unlinked the entry; reinserting it would corrupt
                // the list.
                if entry.wait_queue_link.in_list() {
                    list.reinsert(pkey, entry);
                }
            })
            .map_err(|_| ())
    }
}

impl<L: NestingLock> Default for Notify<L> {
    fn default() -> Notify<L> {
        Notify::new()
    }
}

unsafe impl<L: NestingLock + Sync> Sync for Notify<L> {}
