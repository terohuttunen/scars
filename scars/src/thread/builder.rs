use super::IDLE_THREAD_ID;
use super::{RawThread, Thread, ThreadRef};
use crate::kernel::hal::Context;
use crate::kernel::stack::StackRefMut;
use crate::priority::Priority;
use crate::task::ThreadExecutor;
use crate::tls::{LocalCell, LocalStorage};
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};
use scars_khal::ContextInfo;

static NEXT_FREE_THREAD_ID: AtomicU32 = AtomicU32::new(IDLE_THREAD_ID);

pub struct ThreadBuilder<const PRIO: Priority, F: FnMut() + Send + 'static> {
    thread: &'static mut RawThread,
    closure: &'static mut MaybeUninit<F>,
    stack: StackRefMut,
}

impl<const PRIO: Priority, F: FnMut() + Send + 'static> ThreadBuilder<PRIO, F> {
    pub fn new(
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

    pub fn start<C: FnOnce() -> F>(self, closure: C) -> ThreadRef {
        self.attach(closure).start()
    }

    pub fn build<C: FnOnce() -> F>(self, closure: C) -> ThreadRef {
        self.attach(closure).finish()
    }

    pub fn attach<C: FnOnce() -> F>(self, closure: C) -> InitializedThread {
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

        InitializedThread {
            thread: self.thread,
        }
    }

    pub fn set_local_storage(&mut self, local_storage: LocalStorage) {
        self.thread
            .set_local_storage(local_storage)
            .unwrap_or_else(|_| {
                panic!("TLS already set");
            });
    }

    pub fn start_executor(&mut self, executor: &'static LocalCell<ThreadExecutor>) {
        self.thread.start_executor(executor);
    }

    pub fn modify<R>(&mut self, f: impl FnOnce(&mut RawThread) -> R) -> R {
        f(self.thread)
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

pub struct InitializedThread {
    thread: &'static mut RawThread,
}

impl InitializedThread {
    pub fn start(self) -> ThreadRef {
        let InitializedThread { thread } = self;
        let thread_ref = unsafe { ThreadRef::from_ptr(thread as *const _) };
        unsafe { thread.start() };

        thread_ref
    }

    pub fn finish(self) -> ThreadRef {
        let InitializedThread { thread } = self;
        ThreadRef::new(thread)
    }

    pub fn set_local_storage(&mut self, local_storage: LocalStorage) {
        self.thread
            .set_local_storage(local_storage)
            .unwrap_or_else(|_| {
                panic!("TLS already set");
            });
    }

    pub fn start_executor(&mut self, executor: &'static LocalCell<ThreadExecutor>) {
        self.thread.start_executor(executor);
    }

    pub fn modify<R>(&mut self, f: impl FnOnce(&mut RawThread) -> R) -> R {
        f(self.thread)
    }

    pub fn name(&self) -> &'static str {
        self.thread.name
    }

    pub fn base_priority(&self) -> Priority {
        self.thread.base_priority
    }

    pub fn stack_ref(&self) -> &StackRefMut {
        unsafe { self.thread.stack.assume_init_ref() }
    }
}
