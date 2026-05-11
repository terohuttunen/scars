use crate::error::SimulatorErrorKind;
use crate::interrupt::INTERRUPTS_ENABLED;
use crate::signal::{ALARM_SIGNAL, SYSCALL_SIGNAL};
use core::cell::{Cell, UnsafeCell};
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, Ordering};
use scars_fault::*;
use scars_khal::ContextInfo;

#[unsafe(no_mangle)]
pub(crate) static CURRENT_THREAD_CONTEXT: AtomicPtr<VirtualContext> =
    AtomicPtr::new(core::ptr::null_mut());

pub(crate) fn current_thread_context() -> &'static VirtualContext {
    unsafe { &*CURRENT_THREAD_CONTEXT.load(Ordering::SeqCst) }
}

pub enum VirtualTrap {
    Syscall {
        id: usize,
        args: [usize; 3],
        rval: usize,
    },
    Alarm,
    ServiceCall,
}

pub struct VirtualContext {
    // Access to `resumed` and `suspension` is protected with the `suspension_lock` mutex.
    resumed: UnsafeCell<bool>,
    suspension: UnsafeCell<libc::pthread_cond_t>,
    suspension_lock: UnsafeCell<libc::pthread_mutex_t>,

    pub name: &'static str,
    pub thread_id: libc::pthread_t,
    pub main_fn: *const (),
    pub argument: Option<NonNull<u8>>,

    pub stack_top_ptr: Cell<*const u8>,
}

impl VirtualContext {
    unsafe fn is_resumed(&self) -> bool {
        unsafe { *self.resumed.get() }
    }

    unsafe fn set_resumed(&self, state: bool) {
        unsafe {
            *self.resumed.get() = state;
        }
    }

    pub fn suspend(&self) {
        unsafe {
            if libc::pthread_mutex_lock(self.suspension_lock.get()) != 0 {
                fault!(SimulatorErrorKind::MutexLockFailed {
                    mutex_ptr: self.suspension_lock.get()
                });
            }

            while !self.is_resumed() {
                if libc::pthread_cond_wait(self.suspension.get(), self.suspension_lock.get()) != 0 {
                    fault!(SimulatorErrorKind::CondWaitFailed {
                        cond_ptr: self.suspension.get()
                    });
                }
            }
            self.set_resumed(false);

            if libc::pthread_mutex_unlock(self.suspension_lock.get()) != 0 {
                fault!(SimulatorErrorKind::MutexUnlockFailed {
                    mutex_ptr: self.suspension_lock.get()
                });
            }
        }
    }

    pub fn resume(&self) {
        unsafe {
            libc::pthread_mutex_lock(self.suspension_lock.get());
            if libc::pthread_self() != self.thread_id {
                self.set_resumed(true);
                libc::pthread_cond_signal(self.suspension.get());
            }
            libc::pthread_mutex_unlock(self.suspension_lock.get());
        }
    }
}

impl ContextInfo for VirtualContext {
    fn stack_top_ptr(&self) -> *const u8 {
        self.stack_top_ptr.get()
    }

    unsafe fn init(
        name: &'static str,
        main_fn: *const (),
        argument: Option<*const u8>,
        stack_ptr: *const u8,
        stack_size: usize,
        context: *mut Self,
    ) {
        let mut attr = MaybeUninit::uninit();

        unsafe {
            let stackaddr = stack_ptr.sub(stack_size) as *mut libc::c_void;
            if libc::pthread_attr_init(attr.as_mut_ptr()) != 0
                || libc::pthread_attr_setstack(attr.as_mut_ptr(), stackaddr, stack_size) != 0
            {
                fault!(SimulatorErrorKind::ThreadStackInitFailed { name, stack_size });
            }

            // Initialize thread context variables with `thread_id` field last so that the
            // thread can safely access its context.
            (*context).name = name;
            (*context).resumed = UnsafeCell::new(false);
            (*context).suspension = UnsafeCell::new(libc::PTHREAD_COND_INITIALIZER);
            (*context).suspension_lock = UnsafeCell::new(libc::PTHREAD_MUTEX_INITIALIZER);
            (*context).main_fn = main_fn;
            (*context).argument = argument.map(|a| NonNull::new_unchecked(a as *mut _));
            (*context).stack_top_ptr.set(stack_ptr);

            // Creating the thread initializes the last field of thread context, the `thread_id`.
            libc::pthread_create(
                core::ptr::addr_of_mut!((*context).thread_id),
                attr.as_ptr(),
                thread_main_wrapper,
                context as *mut _,
            );

            libc::pthread_attr_destroy(attr.as_mut_ptr());

            // Set thread name to RTOS thread name
            let c_name = std::ffi::CString::new(name).expect("");
            libc::pthread_setname_np((*context).thread_id, c_name.as_ptr() as *const _);
        }
    }
}

impl core::fmt::Debug for VirtualContext {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Virtual context Debug not implemented")
    }
}

/// Wrapper for the pthread main: suspends until the scheduler resumes
/// the thread, unblocks trap/alarm signals, then enters the RTOS thread
/// body.
extern "C" fn thread_main_wrapper(arg: *mut libc::c_void) -> *mut libc::c_void {
    let context = unsafe { &mut *(arg as *mut VirtualContext) };
    // Wait for resume from trap signal handler
    context.suspend();
    INTERRUPTS_ENABLED.store(true, Ordering::SeqCst);

    unsafe {
        let mut set = MaybeUninit::uninit();
        libc::sigemptyset(set.as_mut_ptr());
        libc::sigaddset(set.as_mut_ptr(), SYSCALL_SIGNAL);
        libc::sigaddset(set.as_mut_ptr(), ALARM_SIGNAL);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, set.as_mut_ptr(), core::ptr::null_mut());
    }

    let main_fn: fn(Option<NonNull<u8>>) = unsafe { core::mem::transmute(context.main_fn) };
    main_fn(context.argument);

    core::ptr::null_mut()
}
