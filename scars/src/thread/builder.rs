use super::IDLE_THREAD_ID;
use super::{RawThread, Thread, ThreadFn, ThreadRef};
use crate::kernel::hal::Context;
use crate::kernel::stack::StackRefMut;
use crate::priority::Priority;
use crate::sync::atomic::{AtomicU32, Ordering};
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use scars_khal::ContextInfo;

static NEXT_FREE_THREAD_ID: AtomicU32 = AtomicU32::new(IDLE_THREAD_ID);

pub struct ThreadBuilder<const PRIO: Priority, F: ThreadFn> {
    thread: &'static mut RawThread,
    closure: &'static mut MaybeUninit<F>,
    stack: StackRefMut,
}

impl<const PRIO: Priority, F: ThreadFn> ThreadBuilder<PRIO, F> {
    pub(crate) fn new(
        thread: &'static mut RawThread,
        closure: &'static mut MaybeUninit<F>,
        stack: StackRefMut,
    ) -> Self {
        Self {
            thread,
            closure,
            stack,
        }
    }

    pub fn attach_old<C: FnOnce() -> F>(self, closure: C) -> ThreadHandle {
        let closure = closure();
        let closure_ref = self.closure.write(closure);
        let closure_ptr = closure_ref as *const F as *const ();

        // A closure cannot be called directly, so every thread has a wrapper function
        // that calls the closure. The wrapper function is passed as the main function
        // to the KHAL thread context. The wrapper function then calls the closure.
        // The closure is passed as an argument to the wrapper function.
        self.thread.main_fn = Thread::<PRIO, F>::closure_wrapper as *const ();

        self.thread.stack.write(self.stack);
        let stack_ptr = unsafe { self.thread.stack.assume_init_ref() }.bottom_ptr();
        let stack_size = unsafe { self.thread.stack.assume_init_ref() }.alloc_size();

        self.thread.thread_id = NEXT_FREE_THREAD_ID.fetch_add(1, Ordering::SeqCst);

        unsafe {
            Context::init(
                self.thread.name,
                self.thread.main_fn,
                Some(closure_ptr as *const u8),
                stack_ptr,
                stack_size,
                self.thread.context.as_mut_ptr(),
            );
        }

        // SAFETY: self.thread is a valid &'static mut from StaticCell
        ThreadHandle {
            thread: NonNull::from(self.thread),
        }
    }

    pub fn attach(self, closure: F) -> ThreadHandle {
        let closure_ref = self.closure.write(closure);
        let closure_ptr = closure_ref as *const F as *const ();

        // A closure cannot be called directly, so every thread has a wrapper function
        // that calls the closure. The wrapper function is passed as the main function
        // to the KHAL thread context. The wrapper function then calls the closure.
        // The closure is passed as an argument to the wrapper function.
        self.thread.main_fn = Thread::<PRIO, F>::closure_wrapper as *const ();

        self.thread.stack.write(self.stack);
        let stack_ptr = unsafe { self.thread.stack.assume_init_ref() }.bottom_ptr();
        let stack_size = unsafe { self.thread.stack.assume_init_ref() }.alloc_size();

        self.thread.thread_id = NEXT_FREE_THREAD_ID.fetch_add(1, Ordering::SeqCst);

        unsafe {
            Context::init(
                self.thread.name,
                self.thread.main_fn,
                Some(closure_ptr as *const u8),
                stack_ptr,
                stack_size,
                self.thread.context.as_mut_ptr(),
            );
        }

        // SAFETY: self.thread is a valid &'static mut from StaticCell
        ThreadHandle {
            thread: NonNull::from(self.thread),
        }
    }

    pub fn name(&self) -> &'static str {
        self.thread.name
    }

    pub fn base_priority(&self) -> Priority {
        self.thread.base_priority
    }

    pub fn as_ref(&self) -> ThreadRef {
        unsafe { ThreadRef::from_ptr(self.thread as *const _) }
    }

    pub fn stack_ref(&self) -> &StackRefMut {
        &self.stack
    }
}

pub struct ThreadHandle {
    thread: NonNull<RawThread>,
}

impl ThreadHandle {
    /// Get a static reference to the raw thread
    ///
    /// # Safety
    /// The NonNull pointer is guaranteed to be valid for 'static lifetime
    /// as it was created from a StaticCell.
    fn raw(&self) -> &'static RawThread {
        // SAFETY: self.thread points to data in a StaticCell with 'static lifetime
        unsafe { self.thread.as_ref() }
    }

    /// Get a mutable reference to the raw thread
    ///
    /// # Safety
    /// The NonNull pointer is guaranteed to be valid for 'static lifetime
    /// as it was created from a StaticCell.
    fn raw_mut(&mut self) -> &'static mut RawThread {
        // SAFETY: self.thread points to data in a StaticCell with 'static lifetime
        // and we have &mut self ensuring exclusive access
        unsafe { self.thread.as_mut() }
    }

    pub fn start(mut self) -> ThreadRef {
        let thread = self.raw_mut();
        let thread_ref = unsafe { ThreadRef::from_ptr(thread as *const _) };
        unsafe { thread.start() };

        thread_ref
    }

    pub(crate) fn modify<R>(&mut self, f: impl FnOnce(&mut RawThread) -> R) -> R {
        f(self.raw_mut())
    }

    pub fn name(&self) -> &'static str {
        self.raw().name
    }

    pub fn base_priority(&self) -> Priority {
        self.raw().base_priority
    }

    pub fn stack_ref(&self) -> &StackRefMut {
        unsafe { self.raw().stack.assume_init_ref() }
    }

    pub fn get_ref(&self) -> ThreadRef {
        unsafe { ThreadRef::from_ptr(self.raw() as *const _) }
    }
}
