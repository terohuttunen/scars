mod builder;
mod raw_thread;
mod reference;

use crate::kernel::hal::CoreId;
use crate::kernel::{Priority, list::LinkedListTag, stack::StackRefMut};
pub use builder::*;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

pub use raw_thread::*;
pub use reference::*;
use static_cell::ConstStaticCell;

#[macro_export]
macro_rules! make_thread {
    ($name: expr, $prio : expr, $stack_size : expr, executor = true $(, core = $core:expr)?) => {{
        let mut thread = $crate::make_thread!($name, $prio, $stack_size $(, core = $core)?);
        let executor = $crate::make_thread_executor!();
        thread.start_executor(executor);
        thread
    }};
    ($name: expr, $prio : expr, $stack_size: expr $(, core = $core:expr)?) => {{
        static STACK: $crate::Stack<{ $stack_size }> = $crate::Stack::new();
        type T = impl ::core::marker::Sized + ::core::marker::Send + FnMut();
        static THREAD: $crate::Thread<{ $prio }, T, { $crate::make_thread!(@core $($core)?) }> =
            $crate::Thread::new($name);
        THREAD.init(STACK.init())
    }};
    (@core) => {$crate::CoreId::DEFAULT};
    (@core $core:expr) => { $core };
}

pub const INVALID_THREAD_ID: u32 = 0;
pub const IDLE_THREAD_ID: u32 = 1;

pub struct LockListTag {}

impl LinkedListTag for LockListTag {}

pub struct InheritanceLockListTag {}

impl LinkedListTag for InheritanceLockListTag {}

pub struct ThreadInfo {
    pub name: &'static str,
    pub state: ThreadExecutionState,
    pub base_priority: Priority,
    pub core: CoreId,
    pub stack_addr: *const (),
    pub stack_size: usize,
    pub entry: *const (),
}

pub trait ThreadFn: FnMut() -> ! + Send + 'static {}

impl<F: FnMut() -> ! + Send + 'static> ThreadFn for F {}

pub struct Thread<const PRIO: Priority, F: ThreadFn, const CORE: CoreId = { CoreId::DEFAULT }> {
    thread: ConstStaticCell<RawThread>,
    closure: UnsafeCell<MaybeUninit<F>>,
}

impl<const PRIO: Priority, F: ThreadFn, const CORE: CoreId> Thread<PRIO, F, CORE> {
    pub const fn new(name: &'static str) -> Thread<PRIO, F, CORE> {
        Thread {
            thread: ConstStaticCell::new(RawThread::new(
                name,
                PRIO,
                CORE,
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
        unsafe {
            RawThread::init_at(thread as *mut _);
        }
        let closure = unsafe { &mut *self.closure.get() };
        ThreadBuilder::new(thread, closure, stack)
    }
}

unsafe impl<const PRIO: Priority, F: ThreadFn, const CORE: CoreId> Sync for Thread<PRIO, F, CORE> {}
