mod builder;
mod raw_thread;
mod reference;

use crate::kernel::{Priority, list::LinkedListTag, stack::StackRefMut, waiter::Suspendable};
pub use builder::*;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

pub use raw_thread::*;
pub use reference::*;
use static_cell::ConstStaticCell;

#[macro_export]
macro_rules! make_thread {
    ($name: expr, $prio : expr, $stack_size : expr, $local_storage_size : expr, executor = true) => {{
        let mut thread = $crate::make_thread!($name, $prio, $stack_size, $local_storage_size);
        let executor = $crate::make_thread_executor!();
        thread.start_executor(executor);
        thread
    }};
    ($name: expr, $prio : expr, $stack_size : expr, $local_storage_size : expr) => {{
        let mut thread = $crate::make_thread!($name, $prio, $stack_size);
        let local_storage = $crate::make_local_storage!($local_storage_size);
        thread.set_local_storage(local_storage);
        thread
    }};
    ($name: expr, $prio : expr, $stack_size: expr) => {{
        static STACK: $crate::Stack<{ $stack_size }> = $crate::Stack::new();
        type T = impl ::core::marker::Sized + ::core::marker::Send + FnMut();
        static THREAD: $crate::Thread<{ $prio }, T> = $crate::Thread::new($name);
        THREAD.init(STACK.init())
    }};
}

pub const INVALID_THREAD_ID: u32 = 0;
pub const IDLE_THREAD_ID: u32 = 1;

pub struct LockListTag {}

impl LinkedListTag for LockListTag {}

pub struct ThreadInfo {
    pub name: &'static str,
    pub state: ThreadExecutionState,
    pub base_priority: Priority,
    pub stack_addr: *const (),
    pub stack_size: usize,
    pub entry: *const (),
}

pub struct Thread<const PRIO: Priority, F: FnMut() + Send> {
    thread: ConstStaticCell<RawThread>,
    closure: UnsafeCell<MaybeUninit<F>>,
}

impl<const PRIO: Priority, F: FnMut() + Send> Thread<PRIO, F> {
    pub const fn new(name: &'static str) -> Thread<PRIO, F> {
        Thread {
            thread: ConstStaticCell::new(RawThread::new(
                name,
                PRIO,
                Self::closure_wrapper as *const (),
            )),
            closure: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    unsafe extern "C" fn closure_wrapper(closure_ptr: *mut ::core::ffi::c_void) {
        let closure = unsafe { &mut *(closure_ptr as *mut F) };
        closure();
    }

    pub fn init(&'static self, stack: StackRefMut) -> ThreadBuilder<PRIO, F> {
        let thread = self.thread.take();
        thread.suspendable = Suspendable::new_thread(thread);
        let closure = unsafe { &mut *self.closure.get() };
        ThreadBuilder::new(thread, closure, stack)
    }
}

unsafe impl<const PRIO: Priority, F: FnMut() + Send> Sync for Thread<PRIO, F> {}
