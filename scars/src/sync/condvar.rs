use crate::interrupt::in_interrupt;
use crate::kernel::hal::CoreId;
use crate::priority::Priority;
use crate::sync::Notify;
use crate::sync::guarded::guard_raw;
use crate::sync::{
    CoreCeilingLock, CoreInterruptLock, CorePreemptLock, LockOps, MutexGuard, NestingLock,
    TimedOut, Unlock,
};
use crate::time::Instant;

pub type Condvar<const CORE: CoreId = { CoreId::DEFAULT }> = LockedCondvar<CorePreemptLock<CORE>>;
pub type CeilingCondvar<const CEILING: Priority, const CORE: CoreId = { CoreId::DEFAULT }> =
    LockedCondvar<CoreCeilingLock<CEILING, CORE>>;
pub type InterruptCondvar<const CORE: CoreId = { CoreId::DEFAULT }> =
    LockedCondvar<CoreInterruptLock<CORE>>;

pub struct LockedCondvar<L: NestingLock> {
    notifier: Notify<L>,
}

impl<L: NestingLock> LockedCondvar<L> {
    pub const fn new() -> LockedCondvar<L> {
        LockedCondvar {
            notifier: Notify::new(),
        }
    }

    #[inline(never)]
    fn wait_lock<G: LockOps>(&self, guard: &mut G::Guard<'_>)
    where
        for<'a> G::Guard<'a>: Unlock,
    {
        // Release the mutex only once queued on the notify: `wait_with`
        // arms first, so a notifier that takes the mutex right after the
        // unlock cannot lose the wakeup.
        self.notifier.wait_with(|| unsafe { guard.unlock() });
        guard.relock();
    }

    #[inline(always)]
    pub fn wait<'a, T, G: LockOps>(&self, mut guard: MutexGuard<'a, T, G>) -> MutexGuard<'a, T, G>
    where
        for<'b> G::Guard<'b>: Unlock,
    {
        if in_interrupt() {
            // Error: cannot wait condition variable in interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        let raw = guard_raw(&mut guard);

        self.wait_lock::<G>(raw);
        guard
    }

    pub fn wait_while<'a, T, G: LockOps, F>(
        &self,
        mut guard: MutexGuard<'a, T, G>,
        mut condition: F,
    ) -> MutexGuard<'a, T, G>
    where
        F: FnMut(&mut T) -> bool,
        for<'b> G::Guard<'b>: Unlock,
    {
        if in_interrupt() {
            // Error: cannot wait condition variable in interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        while condition(&mut *guard) {
            guard = self.wait(guard);
        }
        guard
    }

    #[inline(never)]
    fn wait_lock_until<G: LockOps>(
        &self,
        guard: &mut G::Guard<'_>,
        deadline: Instant,
    ) -> Result<(), TimedOut>
    where
        for<'a> G::Guard<'a>: Unlock,
    {
        // Release the mutex only once queued on the notify; see `wait_lock`.
        let outcome = self
            .notifier
            .wait_until_with(deadline, || unsafe { guard.unlock() });
        guard.relock();
        outcome
    }

    /// Wait until notified or until `deadline` elapses, whichever
    /// comes first. Returns the re-locked guard on success; drops the
    /// guard (releasing the mutex) and returns `Err(TimedOut)` on
    /// deadline expiry.
    #[inline(always)]
    pub fn wait_until<'a, T, G: LockOps>(
        &self,
        mut guard: MutexGuard<'a, T, G>,
        deadline: Instant,
    ) -> Result<MutexGuard<'a, T, G>, TimedOut>
    where
        for<'b> G::Guard<'b>: Unlock,
    {
        if in_interrupt() {
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        let raw = guard_raw(&mut guard);
        self.wait_lock_until::<G>(raw, deadline)?;
        Ok(guard)
    }

    /// Repeatedly wait until either `condition` becomes false or
    /// `deadline` elapses. On timeout, drops the guard (releasing the
    /// mutex) and returns `Err(TimedOut)`.
    pub fn wait_while_until<'a, T, G: LockOps, F>(
        &self,
        mut guard: MutexGuard<'a, T, G>,
        mut condition: F,
        deadline: Instant,
    ) -> Result<MutexGuard<'a, T, G>, TimedOut>
    where
        F: FnMut(&mut T) -> bool,
        for<'b> G::Guard<'b>: Unlock,
    {
        if in_interrupt() {
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        while condition(&mut *guard) {
            guard = self.wait_until(guard, deadline)?;
        }
        Ok(guard)
    }

    pub async fn async_wait<'a, T, G: LockOps>(
        &'static self,
        mut guard: MutexGuard<'static, T, G>,
    ) -> MutexGuard<'static, T, G>
    where
        for<'b> G::Guard<'b>: Unlock,
    {
        let raw = guard_raw(&mut guard);

        unsafe {
            raw.unlock();
        }

        self.notifier.async_wait().await;

        raw.relock();

        guard
    }

    pub async fn async_wait_while<T, G: LockOps, F>(
        &'static self,
        mut guard: MutexGuard<'static, T, G>,
        condition: F,
    ) -> MutexGuard<'static, T, G>
    where
        F: FnOnce(&mut T) -> bool + 'static + core::marker::Copy,
        for<'b> G::Guard<'b>: Unlock,
    {
        while condition(&mut *guard) {
            guard = self.async_wait(guard).await;
        }
        guard
    }

    pub fn notify_one(&self) {
        self.notifier.notify_one()
    }

    pub fn notify_all(&self) {
        self.notifier.notify_all()
    }
}

impl<L: NestingLock> Default for LockedCondvar<L> {
    fn default() -> LockedCondvar<L> {
        LockedCondvar::new()
    }
}
