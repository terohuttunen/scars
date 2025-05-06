use super::TryLockError;
use crate::kernel::{
    list::{Node, impl_linked},
    scheduler::{ExecutionContext, Scheduler},
    waiter::WaitQueue,
};
use crate::runtime_error;
use crate::sync::{PreemptLock, ScopedLock, TryLockResult, Unlock};
use crate::thread::{InheritanceLockListTag, RawThread};
use core::pin::Pin;
use core::sync::atomic::{AtomicPtr, Ordering};

pub struct InheritanceLock {
    // The current owner of the lock
    owner: AtomicOwner,

    // Threads waiting for the lock
    wait_queue: WaitQueue<PreemptLock>,

    // Node for thread lock list.
    // Only one thread owns the lock at any given time, and
    // the thread maintains a list of locks it holds.
    lock_list_node: Node<Self, InheritanceLockListTag>,
}

impl_linked!(lock_list_node, InheritanceLock, InheritanceLockListTag);

impl InheritanceLock {
    pub const fn new() -> Self {
        Self {
            owner: AtomicOwner::new(),
            wait_queue: WaitQueue::new(),
            lock_list_node: Node::new(),
        }
    }

    fn acquire_lock(self: Pin<&Self>) {
        let ExecutionContext::Thread(current_thread) = Scheduler::current_execution_context()
        else {
            runtime_error!(RuntimeError::InterruptHandlerViolation)
        };

        loop {
            match self.owner.take_ownership(current_thread) {
                Ok(_) => {
                    PreemptLock::with(|pkey| unsafe {
                        current_thread.inheritance_lock_acquired(pkey, self);
                    });
                    break;
                }
                Err(owner) => {
                    PreemptLock::with(|pkey| {
                        // When the lock cannot be acquired, the owner of the lock
                        // inherits the priority of the current thread.
                        let current_priority = current_thread.priority(pkey);
                        owner.inherit_priority(pkey, current_priority);
                    });

                    self.wait_queue.wait();
                }
            }
        }
    }

    fn try_acquire_lock(self: Pin<&Self>) -> TryLockResult<()> {
        let ExecutionContext::Thread(current_thread) = Scheduler::current_execution_context()
        else {
            runtime_error!(RuntimeError::InterruptHandlerViolation)
        };

        match self.owner.take_ownership(current_thread) {
            Ok(_) => {
                PreemptLock::with(|pkey| unsafe {
                    current_thread.inheritance_lock_acquired(pkey, self);
                });
                Ok(())
            }
            Err(_) => Err(TryLockError::WouldBlock),
        }
    }

    fn release_lock(self: Pin<&Self>) {
        let ExecutionContext::Thread(current_thread) = Scheduler::current_execution_context()
        else {
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        };

        PreemptLock::with(|pkey| unsafe {
            current_thread.inheritance_lock_released(pkey, self);
        });

        self.owner.release_ownership(current_thread);

        self.wait_queue.notify_one();
    }

    pub fn lock(self: Pin<&Self>) -> InheritanceLockGuard<'_> {
        self.acquire_lock();

        InheritanceLockGuard { lock: self }
    }

    pub fn try_lock(self: Pin<&Self>) -> TryLockResult<InheritanceLockGuard<'_>> {
        self.try_acquire_lock()?;

        Ok(InheritanceLockGuard { lock: self })
    }
}

unsafe impl Send for InheritanceLock {}
unsafe impl Sync for InheritanceLock {}

impl ScopedLock for InheritanceLock {
    type Guard<'lock> = InheritanceLockGuard<'lock>;

    fn lock(&self) -> Self::Guard<'_> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.lock()
    }

    fn try_lock(&self) -> TryLockResult<Self::Guard<'_>> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.try_lock()
    }
}

pub struct InheritanceLockGuard<'lock> {
    lock: Pin<&'lock InheritanceLock>,
}

impl<'lock> Drop for InheritanceLockGuard<'lock> {
    fn drop(&mut self) {
        self.lock.release_lock();
    }
}

impl<'lock> Unlock for InheritanceLockGuard<'lock> {
    unsafe fn unlock(&mut self) {
        self.lock.release_lock();
    }

    fn relock(&mut self) {
        self.lock.acquire_lock();
    }
}

pub struct AtomicOwner {
    owner: AtomicPtr<RawThread>,
}

impl AtomicOwner {
    pub const fn new() -> Self {
        Self {
            owner: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    pub fn take_ownership(
        &self,
        current_thread: Pin<&'static RawThread>,
    ) -> Result<(), Pin<&'static RawThread>> {
        let current_thread = current_thread.get_ref() as *const _ as *mut _;
        self.owner
            .compare_exchange(
                core::ptr::null_mut(),
                current_thread,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .map(|_| ())
            .map_err(|owner| {
                if owner == current_thread {
                    crate::runtime_error!(RuntimeError::RecursiveLock);
                }
                unsafe { Pin::new_unchecked(&*owner) }
            })
    }

    pub fn release_ownership(
        &self,
        current_thread: Pin<&'static RawThread>,
    ) -> Pin<&'static RawThread> {
        if self.owner.load(Ordering::Relaxed) != current_thread.get_ref() as *const _ as *mut _ {
            crate::runtime_error!(RuntimeError::LockOwnerViolation);
        }

        let owner = self.owner.swap(core::ptr::null_mut(), Ordering::Release);

        unsafe { Pin::new_unchecked(&*owner) }
    }
}
