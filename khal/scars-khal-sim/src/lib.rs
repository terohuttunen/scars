#![feature(sync_unsafe_cell)]
#![feature(linkage)]
extern crate libc;
extern crate std;
use core::cell::{Cell, UnsafeCell};
use core::mem::MaybeUninit;
use core::ptr::{NonNull, addr_of_mut};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};
use scars_khal::*;

pub mod pac {
    pub enum Interrupt {
        UART1,
    }
}

#[macro_use]
pub mod printk;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct InterruptVector {
    pub handler_ptr: *const (),
    pub locals_ptr: *const u8,
}

#[unsafe(no_mangle)]
static mut __EXTERNAL_INTERRUPTS: [InterruptVector; MAX_INTERRUPT + 1] = [InterruptVector {
    handler_ptr: core::ptr::null(),
    locals_ptr: core::ptr::null(),
}; MAX_INTERRUPT + 1];

#[derive(PartialEq, Eq, Copy, Clone)]
pub enum FaultKind {
    Unknown = 255,
}

impl TryFrom<usize> for FaultKind {
    type Error = ();
    fn try_from(value: usize) -> Result<FaultKind, ()> {
        match value {
            _ => Err(()),
        }
    }
}

impl FaultKind {
    pub fn name(&self) -> &'static str {
        match self {
            FaultKind::Unknown => "Unknown",
        }
    }
}

pub struct Fault {
    kind: FaultKind,
}

impl Fault {
    pub fn new(kind: FaultKind) -> Fault {
        Fault { kind }
    }
}

impl FaultInfo<VirtualContext> for Fault {
    fn code(&self) -> usize {
        self.kind as usize
    }

    fn name(&self) -> &'static str {
        self.kind.name()
    }

    fn address(&self) -> usize {
        unimplemented!()
    }

    fn context(&self) -> &VirtualContext {
        unimplemented!()
    }
}

pub struct VirtualContext {
    // Access to `interrupts_enabled`, `resumed` and `suspension` is protected with the `suspension_lock` mutex.
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
                panic!("");
            }

            while !self.is_resumed() {
                if libc::pthread_cond_wait(self.suspension.get(), self.suspension_lock.get()) != 0 {
                    panic!("");
                }
            }
            self.set_resumed(false);

            if libc::pthread_mutex_unlock(self.suspension_lock.get()) != 0 {
                panic!("");
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
                panic!(
                    "Failed to set thread '{}' stack to {} bytes",
                    name, stack_size
                );
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

pub enum VirtualTrap {
    Syscall {
        id: usize,
        args: [usize; 3],
        rval: usize,
    },
    Alarm,
    // Interrupt {}
}

#[unsafe(no_mangle)]
static CURRENT_THREAD_CONTEXT: AtomicPtr<VirtualContext> = AtomicPtr::new(core::ptr::null_mut());

impl FlowController for Simulator {
    type StackAlignment = A16;
    type Context = VirtualContext;
    type Fault = Fault;

    fn start_first_thread(context: *mut Self::Context) -> ! {
        let mut wait_set = MaybeUninit::uninit();
        let mut sig = MaybeUninit::uninit();
        // Only the currently active RTOS thread should receive the virtual
        // trap signal SIGUSR1 and alarm signal SIGALRM.
        unsafe {
            libc::sigemptyset(wait_set.as_mut_ptr());
            libc::sigaddset(wait_set.as_mut_ptr(), libc::SIGUSR1);
            libc::sigaddset(wait_set.as_mut_ptr(), libc::SIGALRM);
            libc::pthread_sigmask(
                libc::SIG_BLOCK,
                wait_set.as_mut_ptr(),
                core::ptr::null_mut(),
            );
        }

        CURRENT_THREAD_CONTEXT.store(context, Ordering::SeqCst);

        unsafe { &*context }.resume();
        loop {
            // Signals SIGUSR1 and SIGALRM, which the RTOS threads use for traps and alarm
            // clock are blocked from this thread, and should always be handled by the
            // currently running RTOS thread.
            unsafe {
                libc::sigwait(wait_set.as_ptr(), sig.as_mut_ptr());
            }
        }
    }

    fn abort() -> ! {
        unsafe {
            libc::exit(1);
        }
    }

    fn breakpoint() {
        unimplemented!()
    }

    #[inline(always)]
    fn idle() {
        unsafe {
            libc::sched_yield();
        }
    }

    fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
        let mut trap = VirtualTrap::Syscall {
            id,
            args: [arg0, arg1, arg2],
            rval: 0,
        };
        let context = current_thread_context();
        if unsafe {
            libc::pthread_sigqueue(context.thread_id, libc::SIGUSR1, libc::sigval {
                sival_ptr: &mut trap as *mut VirtualTrap as *mut std::ffi::c_void,
            })
        } != 0
        {
            panic!("");
        }
        if let VirtualTrap::Syscall { rval, .. } = trap {
            rval
        } else {
            unreachable!()
        }
    }

    fn current_thread_context() -> *const VirtualContext {
        unsafe { &*CURRENT_THREAD_CONTEXT.load(Ordering::SeqCst) }
    }

    fn set_current_thread_context(context: *const VirtualContext) {
        CURRENT_THREAD_CONTEXT.store(context as *mut _, Ordering::SeqCst);
    }
}

fn current_thread_context() -> &'static VirtualContext {
    unsafe { &*Simulator::current_thread_context() }
}

// Wrapper for thread main that suspends the thead until it is
// resumed in the trap handler. Otherwise the thread would start
// executing the thread main before the scheduler has been able
// to initialize and switch into the thread.
extern "C" fn thread_main_wrapper(arg: *mut libc::c_void) -> *mut libc::c_void {
    let context = unsafe { &mut *(arg as *mut VirtualContext) };
    // Wait for resume from trap signal handler
    context.suspend();
    INTERRUPTS_ENABLED.store(true, Ordering::SeqCst);

    unsafe {
        let mut set = MaybeUninit::uninit();
        libc::sigemptyset(set.as_mut_ptr());
        libc::sigaddset(set.as_mut_ptr(), libc::SIGUSR1);
        libc::sigaddset(set.as_mut_ptr(), libc::SIGALRM);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, set.as_mut_ptr(), core::ptr::null_mut());
    }

    let main_fn: fn(Option<NonNull<u8>>) = unsafe { core::mem::transmute(context.main_fn) };

    main_fn(context.argument);

    core::ptr::null_mut()
}

extern "C" fn trap_signal_handler(
    sig: libc::c_int,
    info: *const libc::siginfo_t,
    ucontext: *const libc::ucontext_t,
) {
    // thread that got interrupted by the signal
    let interrupted_context = current_thread_context();

    // Disable interrupts for the duration of the trap handling
    let restore_state = INTERRUPTS_ENABLED.swap(false, Ordering::SeqCst);

    match sig {
        // Virtual software interrupt signals
        libc::SIGUSR1 => {
            let trap = unsafe { &mut *((*info).si_value().sival_ptr as *mut VirtualTrap) };
            VirtualInterruptController::handle_trap(trap);
        }
        // Virtual timer interrupt signals
        libc::SIGALRM | libc::SIGUSR2 => {
            VirtualTimer::handle_alarm();
        }
        _ => panic!("Unhandled exception"),
    }

    // If the current thread has been changed by the trap handling,
    // resume the new current thread, and suspend the thread that was
    // interrupted.
    let context_to_resume = current_thread_context();
    if context_to_resume.thread_id != interrupted_context.thread_id {
        context_to_resume.resume();
        interrupted_context
            .stack_top_ptr
            .set(unsafe { (*ucontext).uc_stack.ss_sp as *const u8 });
        interrupted_context.suspend();
    }

    // Restore interrupts enable state
    INTERRUPTS_ENABLED.store(restore_state, Ordering::SeqCst);
}

pub const TIMER_FREQ_HZ: u64 = 10_000_000;

pub const MAX_INTERRUPT_PRIORITY: u8 =
    <Simulator as InterruptController>::MAX_INTERRUPT_PRIORITY as u8;
pub const MAX_INTERRUPT: usize = <Simulator as InterruptController>::MAX_INTERRUPT_NUMBER;

const INITIAL_PRIORITY: AtomicU8 = AtomicU8::new(0);
const INITIAL_ENABLE: AtomicBool = AtomicBool::new(false);
const INITIAL_STATUS: AtomicBool = AtomicBool::new(false);

static INTERRUPTS_ENABLED: AtomicBool = AtomicBool::new(false);

pub struct VirtualInterruptController {
    priority: [AtomicU8; MAX_INTERRUPT + 1],
    threshold: AtomicU8,
    enable: [AtomicBool; MAX_INTERRUPT + 1],
    _status: [AtomicBool; MAX_INTERRUPT + 1],
    interrupt_sigmask: libc::sigset_t,
}

impl VirtualInterruptController {
    pub fn new() -> VirtualInterruptController {
        let mut interrupt_sigmask = MaybeUninit::uninit();
        unsafe {
            // TBD: any other signals?
            if libc::sigemptyset(interrupt_sigmask.as_mut_ptr()) != 0
                || libc::sigaddset(interrupt_sigmask.as_mut_ptr(), libc::SIGUSR1) != 0
                || libc::sigaddset(interrupt_sigmask.as_mut_ptr(), libc::SIGUSR2) != 0
                || libc::sigaddset(interrupt_sigmask.as_mut_ptr(), libc::SIGALRM) != 0
            {
                panic!("");
            }

            // Additional signals to mask for SIGUSR1
            // TBD: should be interrupt_sigmask without SIGUSR1,
            // maybe SIGUSR1 in it doesn't do harm?
            let mut mask = MaybeUninit::uninit();
            if libc::sigemptyset(mask.as_mut_ptr()) != 0
                || libc::sigaddset(mask.as_mut_ptr(), libc::SIGALRM) != 0
            {
                panic!("");
            }

            let sigaction = libc::sigaction {
                sa_sigaction: trap_signal_handler as libc::sighandler_t,
                sa_mask: mask.assume_init(),
                sa_flags: libc::SA_SIGINFO,
                sa_restorer: None,
            };
            if libc::sigaction(libc::SIGUSR1, &sigaction, std::ptr::null_mut()) != 0 {
                panic!("Failed to set trap signal handler");
            }
        }
        VirtualInterruptController {
            priority: [INITIAL_PRIORITY; MAX_INTERRUPT + 1],
            threshold: AtomicU8::new(0),
            enable: [INITIAL_ENABLE; MAX_INTERRUPT + 1],
            _status: [INITIAL_STATUS; MAX_INTERRUPT + 1],
            interrupt_sigmask: unsafe { interrupt_sigmask.assume_init() },
        }
    }

    fn handle_trap(trap: &mut VirtualTrap) {
        match trap {
            &mut VirtualTrap::Syscall {
                ref id,
                ref args,
                ref mut rval,
            } => {
                *rval =
                    unsafe { Simulator::kernel_syscall_handler(*id, args[0], args[1], args[2]) };
            }
            _ => panic!("Unhandled exception"),
        }
    }
}

pub struct InterruptClaim {
    interrupt_number: u16,
}

impl GetInterruptNumber for InterruptClaim {
    fn get_interrupt_number(&self) -> u16 {
        self.interrupt_number
    }
}

impl InterruptController for Simulator {
    const MAX_INTERRUPT_PRIORITY: usize = 7;
    const MAX_INTERRUPT_NUMBER: usize = 0;
    type InterruptClaim = InterruptClaim;

    fn get_interrupt_priority(&self, interrupt_number: u16) -> u8 {
        self.interrupt_controller.priority[interrupt_number as usize].load(Ordering::SeqCst)
    }

    fn set_interrupt_priority(&self, interrupt_number: u16, prio: u8) -> u8 {
        self.interrupt_controller.priority[interrupt_number as usize].swap(prio, Ordering::SeqCst)
    }

    fn enable_interrupts(&self) {
        INTERRUPTS_ENABLED.store(true, Ordering::SeqCst);
        unsafe {
            libc::pthread_sigmask(
                libc::SIG_UNBLOCK,
                &self.interrupt_controller.interrupt_sigmask,
                core::ptr::null_mut(),
            );
        }
    }

    fn disable_interrupts(&self) {
        INTERRUPTS_ENABLED.store(false, Ordering::SeqCst);
        unsafe {
            libc::pthread_sigmask(
                libc::SIG_BLOCK,
                &self.interrupt_controller.interrupt_sigmask,
                core::ptr::null_mut(),
            );
        }
    }

    #[inline(always)]
    fn get_interrupt_threshold(&self) -> u8 {
        self.interrupt_controller.threshold.load(Ordering::SeqCst)
    }

    #[inline(always)]
    fn set_interrupt_threshold(&self, threshold: u8) {
        self.interrupt_controller
            .threshold
            .store(threshold, Ordering::SeqCst)
        // TODO: if threshold was decreased, pending interrupts might
        // become executable, if they are enabled.
    }

    fn claim_interrupt(&self) -> InterruptClaim {
        unimplemented!()
    }

    fn complete_interrupt(&self, _claim: InterruptClaim) {
        unimplemented!()
    }

    fn enable_interrupt(&self, interrupt_number: u16) {
        self.interrupt_controller.enable[interrupt_number as usize].store(true, Ordering::SeqCst);
        // TODO: if interrupt is pending, it must be executed
    }

    fn disable_interrupt(&self, interrupt_number: u16) {
        self.interrupt_controller.enable[interrupt_number as usize].store(false, Ordering::SeqCst);
    }

    #[inline(always)]
    fn interrupt_status(&self) -> bool {
        INTERRUPTS_ENABLED.load(Ordering::SeqCst)
    }

    fn acquire(&self) -> bool {
        let old_state = INTERRUPTS_ENABLED.swap(false, Ordering::SeqCst);
        if old_state {
            unsafe {
                libc::pthread_sigmask(
                    libc::SIG_BLOCK,
                    &self.interrupt_controller.interrupt_sigmask,
                    core::ptr::null_mut(),
                );
            }
        }
        old_state
    }

    fn restore(&self, restore_state: bool) {
        // Only re-enable interrupts if they were enabled before the critical section.
        if restore_state {
            INTERRUPTS_ENABLED.swap(true, Ordering::SeqCst);
            unsafe {
                libc::pthread_sigmask(
                    libc::SIG_UNBLOCK,
                    &self.interrupt_controller.interrupt_sigmask,
                    core::ptr::null_mut(),
                );
            }
        }
    }
}

pub struct VirtualTimer {
    timer_id: libc::timer_t,
}

impl VirtualTimer {
    pub fn new() -> VirtualTimer {
        let mut timer_id = MaybeUninit::uninit();

        unsafe {
            let mut mask = MaybeUninit::uninit();
            if libc::sigemptyset(mask.as_mut_ptr()) != 0
                || libc::sigaddset(mask.as_mut_ptr(), libc::SIGUSR1) != 0
            {
                panic!("");
            }
            let sigaction = libc::sigaction {
                sa_sigaction: trap_signal_handler as libc::sighandler_t,
                sa_mask: mask.assume_init(),
                sa_flags: libc::SA_SIGINFO,
                sa_restorer: None,
            };

            if libc::sigaction(libc::SIGUSR2, &sigaction, std::ptr::null_mut()) != 0 {
                panic!("");
            }

            static mut ALARM_TRAP: VirtualTrap = VirtualTrap::Alarm;

            let mut sevp: libc::sigevent = MaybeUninit::zeroed().assume_init();
            sevp.sigev_notify = libc::SIGEV_SIGNAL;
            sevp.sigev_signo = libc::SIGUSR2;
            sevp.sigev_value.sival_ptr =
                addr_of_mut!(ALARM_TRAP) as *const VirtualTrap as *mut libc::c_void;
            if libc::timer_create(libc::CLOCK_MONOTONIC, &mut sevp, timer_id.as_mut_ptr()) != 0 {
                panic!("Failed to create timer");
            }
        }

        VirtualTimer {
            timer_id: unsafe { timer_id.assume_init() },
        }
    }

    fn enabled_flag() -> &'static AtomicBool {
        static TIMER_INTERRUPT_ENABLE: AtomicBool = AtomicBool::new(false);
        &TIMER_INTERRUPT_ENABLE
    }

    fn timespec_to_ticks(time: libc::timespec) -> u64 {
        (time.tv_sec as u64) * TIMER_FREQ_HZ + (time.tv_nsec as u64) * TIMER_FREQ_HZ / 1_000_000_000
    }

    fn ticks_to_timespec(ticks: u64) -> libc::timespec {
        libc::timespec {
            tv_sec: (ticks / TIMER_FREQ_HZ) as i64,
            tv_nsec: ((ticks % TIMER_FREQ_HZ) * (1_000_000_000 / TIMER_FREQ_HZ)) as i64,
        }
    }

    fn handle_alarm() {
        unsafe { Simulator::kernel_wakeup_handler() };
    }
}

impl AlarmClockController for Simulator {
    const TICK_FREQ_HZ: u64 = TIMER_FREQ_HZ;

    fn clock_ticks(&self) -> u64 {
        let mut time = core::mem::MaybeUninit::uninit();
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, time.as_mut_ptr()) } != 0 {
            panic!("Error: failed to read monotonic clock");
        }

        let time = unsafe { time.assume_init() };
        VirtualTimer::timespec_to_ticks(time)
    }

    fn set_wakeup(&self, at: u64) {
        let new_value = libc::itimerspec {
            it_value: VirtualTimer::ticks_to_timespec(at),
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
        };
        let mut old_value = core::mem::MaybeUninit::uninit();
        if unsafe {
            libc::timer_settime(
                self.timer.timer_id,
                libc::TIMER_ABSTIME,
                &new_value,
                old_value.as_mut_ptr(),
            )
        } != 0
        {
            panic!("");
        }
    }

    #[inline(always)]
    fn enable_wakeup(&self) {
        VirtualTimer::enabled_flag().store(true, Ordering::SeqCst);
    }

    #[inline(always)]
    fn disable_wakeup(&self) {
        VirtualTimer::enabled_flag().store(false, Ordering::SeqCst);
    }
}

#[unsafe(no_mangle)]
fn main() {
    unsafe {
        start_kernel();
    }
}

#[unsafe(no_mangle)]
#[linkage = "weak"]
fn _scars_idle_thread_hook() {
    Simulator::idle();
}

pub type HAL = Simulator;

pub struct Simulator {
    timer: VirtualTimer,
    interrupt_controller: VirtualInterruptController,
}

unsafe impl Sync for Simulator {}

impl HardwareAbstractionLayer for Simulator {
    const NAME: &'static str = "Simulator";

    unsafe fn init(hal: *mut Self) {
        unsafe {
            *hal = Simulator {
                timer: VirtualTimer::new(),
                interrupt_controller: VirtualInterruptController::new(),
            }
        }
    }
}
