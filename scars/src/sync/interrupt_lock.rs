use super::TryLockError;
use crate::kernel::hal::{acquire, restore};
use crate::sync::{NestingLock, ScopedLock};
use core::marker::PhantomData;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static INTERRUPT_LOCK_NESTING: AtomicUsize = AtomicUsize::new(0);

pub struct InterruptLock {
    owned: AtomicBool,
}

impl InterruptLock {
    pub fn new() -> InterruptLock {
        InterruptLock {
            owned: AtomicBool::new(false),
        }
    }

    // SAFETY: The caller must ensure that the lock has a corresponding
    // release with `release_scoped_lock`.
    unsafe fn acquire_scoped_lock(self: Pin<&Self>) {
        acquire();
        // Disable interrupts before incrementing the nesting count, to
        // avoid a race between count increment and disabling interrupts.
        // If interrupts were disabled only when previous count was 0,
        // the thread that sees previous count == 1, might run without
        // disabling interrupts.
        INTERRUPT_LOCK_NESTING.fetch_add(1, Ordering::Acquire);

        // The lock cannot be acquired recursively
        if self.owned.swap(true, Ordering::Relaxed) {
            crate::runtime_error!(RuntimeError::RecursiveLock);
        }
    }

    unsafe fn release_scoped_lock(self: Pin<&Self>) {
        // Subtract locking count only once per lock. If the lock is released
        // manually through the guard, release will be called again at guard drop.
        if self.owned.swap(false, Ordering::Relaxed) {
            if INTERRUPT_LOCK_NESTING.fetch_sub(1, Ordering::Release) == 1 {
                restore(true);
            }
        }
    }

    pub fn lock(&self) -> InterruptLockGuard<'_> {
        let this = unsafe { Pin::new_unchecked(self) };
        unsafe {
            this.acquire_scoped_lock();
        }
        InterruptLockGuard { lock: this }
    }

    pub fn try_lock(&self) -> Result<InterruptLockGuard<'_>, TryLockError> {
        Ok(self.lock())
    }

    pub fn with<R>(f: impl FnOnce(InterruptLockKey<'_>) -> R) -> R {
        acquire();
        INTERRUPT_LOCK_NESTING.fetch_add(1, Ordering::Acquire);
        let key = unsafe { InterruptLockKey::new() };
        let result = f(key);
        if INTERRUPT_LOCK_NESTING.fetch_sub(1, Ordering::Release) == 1 {
            restore(true);
        }
        result
    }

    pub fn try_with<R>(f: impl FnOnce(InterruptLockKey<'_>) -> R) -> Result<R, TryLockError> {
        Ok(Self::with(f))
    }
}

impl ScopedLock for InterruptLock {
    type Guard<'lock> = InterruptLockGuard<'lock>;

    fn lock(&self) -> Self::Guard<'_> {
        self.lock()
    }

    fn try_lock(&self) -> Result<Self::Guard<'_>, TryLockError> {
        self.try_lock()
    }
}

impl NestingLock for InterruptLock {
    type Key<'a> = InterruptLockKey<'a>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Self::try_with(f)
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a> {
        unsafe { InterruptLockKey::new() }
    }
}

pub struct InterruptLockGuard<'lock> {
    lock: Pin<&'lock InterruptLock>,
}

impl<'lock> Drop for InterruptLockGuard<'lock> {
    fn drop(&mut self) {
        unsafe {
            self.lock.release_scoped_lock();
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InterruptLockKey<'lock> {
    _private: PhantomData<&'lock ()>,
}

impl InterruptLockKey<'_> {
    /// Creates an interrupt lock token.
    #[inline(always)]
    pub unsafe fn new() -> Self {
        InterruptLockKey {
            _private: PhantomData,
        }
    }
}
