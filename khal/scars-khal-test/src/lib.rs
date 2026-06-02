//! Synchronous test-harness KHAL.
//!
//! A HAL backend that models the kernel's hardware view as in-process
//! state: the clock is a counter, interrupts are a pending bitmap, and a
//! context switch records the selected context rather than transferring
//! control. Tests advance the clock, pend and [`pump`] virtual
//! interrupts, or invoke syscalls, then assert on the recorded state via
//! [`state`].
//!
//! Because a switch is recorded rather than executed, thread bodies do
//! not run. The harness is scoped to deterministic kernel-logic and
//! hardware-introspection tests that do not start threads — clock and
//! alarm behaviour, the interrupt controller, and handlers reached
//! through [`pump`]. Thread lifecycle, preemptive scheduling, and
//! priority inheritance are exercised by the simulator backend, which
//! runs threads for real.
//!
//! Boot reaches the test runner through one concession to execution:
//! `start_first_thread` runs the idle context's `main_fn` on the host
//! stack, so `idle()` -> `test_main()` runs the suite in idle context.
#![feature(linkage)]

use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, Ordering};
use scars_khal::*;
use std::sync::Mutex;

mod context;
mod error;
mod flow;
mod interrupt;
pub mod pac;
mod timer;

pub use context::TestContext;
pub use error::{TestError, TestErrorKind};
pub use interrupt::{InterruptClaim, MAX_INTERRUPT, MAX_INTERRUPT_PRIORITY, pend_interrupt};
pub use timer::TICK_FREQ_HZ;

// The kernel logs with `printkln!`; route it to stdout. Every call site
// uses standard `{}` formatting, so `std::println` handles them.
pub use std::println as printk;
pub use std::println as printkln;

/// The concrete HAL type the kernel binds as `kernel_hal::HAL`.
pub type HAL = TestHal;

/// Number of virtual IRQ slots (`0..=MAX_INTERRUPT_NUMBER`).
pub(crate) const NUM_IRQ: usize = 32;

/// Hardware-visible state of one IRQ line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrqState {
    pub enabled: bool,
    pub priority: u8,
}

/// One recorded context switch: the thread name and context address the
/// kernel selected via `set_current_thread_context`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchEntry {
    pub name: &'static str,
    pub addr: usize,
}

/// Snapshot of the harness "hardware" — the assertion surface for tests.
#[derive(Clone, Debug)]
pub struct HwState {
    pub now: u64,
    pub wakeup: Option<u64>,
    pub wakeups_fired: u32,
    pub threshold: u8,
    pub pending: u32,
    pub interrupts_enabled: bool,
    pub service_pending: bool,
    pub current_thread: Option<&'static str>,
    pub current_context: usize,
    pub switch_log: Vec<SwitchEntry>,
    pub irq: [IrqState; NUM_IRQ],
}

/// Synchronous mock HAL. All state is plain atomics plus a mutex-guarded
/// switch log; the harness is single-threaded, so this is contention-free
/// in practice and needs no `unsafe impl Sync`.
pub struct TestHal {
    // Alarm clock.
    now: AtomicU64,
    wakeup: AtomicU64,
    wakeups_fired: AtomicU32,
    // Interrupt controller.
    priority: [AtomicU8; NUM_IRQ],
    threshold: AtomicU8,
    enable: [AtomicBool; NUM_IRQ],
    pending: AtomicU32,
    interrupts_enabled: AtomicBool,
    // Core / flow.
    current_context: AtomicPtr<TestContext>,
    service_pending: AtomicBool,
    manual_pump: AtomicBool,
    switch_log: Mutex<Vec<SwitchEntry>>,
}

impl TestHal {
    const fn new() -> Self {
        const ZERO_U8: AtomicU8 = AtomicU8::new(0);
        const FALSE: AtomicBool = AtomicBool::new(false);
        TestHal {
            now: AtomicU64::new(0),
            wakeup: AtomicU64::new(timer::NO_WAKEUP),
            wakeups_fired: AtomicU32::new(0),
            priority: [ZERO_U8; NUM_IRQ],
            threshold: AtomicU8::new(0),
            enable: [FALSE; NUM_IRQ],
            pending: AtomicU32::new(0),
            interrupts_enabled: AtomicBool::new(false),
            current_context: AtomicPtr::new(core::ptr::null_mut()),
            service_pending: AtomicBool::new(false),
            manual_pump: AtomicBool::new(false),
            switch_log: Mutex::new(Vec::new()),
        }
    }
}

static HAL_INSTANCE: TestHal = TestHal::new();

#[inline(always)]
pub(crate) fn hal() -> &'static TestHal {
    &HAL_INSTANCE
}

impl HardwareAbstractionLayer for TestHal {
    const NAME: &'static str = "TestHarness";

    fn instance() -> &'static Self {
        &HAL_INSTANCE
    }

    unsafe fn init(_hal: *mut Self) {
        // State is const-initialized. Tests call `reset` between cases to
        // return the harness to a clean state.
    }
}

// ---------------------------------------------------------------------------
// Boot entry. The kernel is `no_main` under test, so the `main` symbol comes
// from the HAL (as with the simulator). Booting reaches the test runner via
// the idle thread (`idle()` -> `test_main()`).
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
fn main() {
    unsafe { start_kernel() }
}

/// Weak idle hook required by the kernel's idle loop. Never reached in
/// practice — the idle body exits via `test_succeed` before looping — but
/// the symbol must resolve at link time.
#[unsafe(no_mangle)]
#[linkage = "weak"]
fn _scars_idle_thread_hook() {
    TestHal::on_idle();
}

// ---------------------------------------------------------------------------
// Internal helpers shared with the trait-impl modules.
// ---------------------------------------------------------------------------

pub(crate) fn record_switch(context: *const TestContext) {
    let entry = if context.is_null() {
        SwitchEntry {
            name: "<null>",
            addr: 0,
        }
    } else {
        SwitchEntry {
            name: unsafe { (*context).name },
            addr: context as usize,
        }
    };
    if let Ok(mut log) = hal().switch_log.lock() {
        log.push(entry);
    }
}

/// Run pending service calls to completion (the deferred PendSV-equivalent
/// work). Mirrors the simulator's service-call trap, synchronously.
pub(crate) fn drain_service_calls() {
    while hal().service_pending.load(Ordering::SeqCst) {
        TestHal::clear_service_call();
        unsafe { TestHal::kernel_service_call_handler() };
    }
}

// ---------------------------------------------------------------------------
// Public driving + introspection API.
// ---------------------------------------------------------------------------

/// Drive every runnable virtual interrupt, then any pending service call,
/// into the kernel until the harness is quiescent.
pub fn pump() {
    loop {
        let mut progressed = false;
        while interrupt::find_runnable().is_some() {
            unsafe { TestHal::kernel_interrupt_handler() };
            progressed = true;
        }
        if hal().service_pending.load(Ordering::SeqCst) {
            TestHal::clear_service_call();
            unsafe { TestHal::kernel_service_call_handler() };
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
}

/// Advance the monotonic clock to `ticks` (never moves backwards). If the
/// armed wakeup deadline is crossed, fire the kernel wakeup handler.
pub fn advance_to(ticks: u64) {
    let h = hal();
    let now = h.now.load(Ordering::SeqCst);
    let target = ticks.max(now);
    h.now.store(target, Ordering::SeqCst);
    let wakeup = h.wakeup.load(Ordering::SeqCst);
    if wakeup != timer::NO_WAKEUP && target >= wakeup {
        h.wakeup.store(timer::NO_WAKEUP, Ordering::SeqCst);
        h.wakeups_fired.fetch_add(1, Ordering::SeqCst);
        unsafe { TestHal::kernel_wakeup_handler() };
        if !h.manual_pump.load(Ordering::SeqCst) {
            drain_service_calls();
        }
    }
}

/// Advance the clock by `delta` ticks.
pub fn advance(delta: u64) {
    let now = hal().now.load(Ordering::SeqCst);
    advance_to(now.saturating_add(delta));
}

/// When `true`, `syscall`/`advance` no longer auto-drain service calls, so
/// a test can observe the pre-switch state and call [`pump`] itself.
pub fn set_manual_pump(manual: bool) {
    hal().manual_pump.store(manual, Ordering::SeqCst);
}

/// Reset the modeled *peripheral* hardware to its post-boot defaults:
/// clock, alarm, interrupt controller, service-call flag, and switch log.
///
/// Deliberately leaves `current_context` untouched — it is coupled to the
/// kernel scheduler, and nulling it would crash any subsequent kernel call
/// that reads the current thread. Resetting kernel scheduler state for
/// full per-test isolation is a separate concern (see the test-isolation
/// notes).
pub fn reset() {
    let h = hal();
    h.now.store(0, Ordering::SeqCst);
    h.wakeup.store(timer::NO_WAKEUP, Ordering::SeqCst);
    h.wakeups_fired.store(0, Ordering::SeqCst);
    for p in h.priority.iter() {
        p.store(0, Ordering::SeqCst);
    }
    for e in h.enable.iter() {
        e.store(false, Ordering::SeqCst);
    }
    h.threshold.store(0, Ordering::SeqCst);
    h.pending.store(0, Ordering::SeqCst);
    h.interrupts_enabled.store(false, Ordering::SeqCst);
    h.service_pending.store(false, Ordering::SeqCst);
    h.manual_pump.store(false, Ordering::SeqCst);
    if let Ok(mut log) = h.switch_log.lock() {
        log.clear();
    }
}

/// Snapshot the current harness "hardware" state for assertions.
pub fn state() -> HwState {
    let h = hal();
    let wakeup = h.wakeup.load(Ordering::SeqCst);
    let cur = h.current_context.load(Ordering::SeqCst);
    let current_thread = if cur.is_null() {
        None
    } else {
        Some(unsafe { (*cur).name })
    };
    let mut irq = [IrqState {
        enabled: false,
        priority: 0,
    }; NUM_IRQ];
    for (i, s) in irq.iter_mut().enumerate() {
        s.enabled = h.enable[i].load(Ordering::SeqCst);
        s.priority = h.priority[i].load(Ordering::SeqCst);
    }
    HwState {
        now: h.now.load(Ordering::SeqCst),
        wakeup: if wakeup == timer::NO_WAKEUP {
            None
        } else {
            Some(wakeup)
        },
        wakeups_fired: h.wakeups_fired.load(Ordering::SeqCst),
        threshold: h.threshold.load(Ordering::SeqCst),
        pending: h.pending.load(Ordering::SeqCst),
        interrupts_enabled: h.interrupts_enabled.load(Ordering::SeqCst),
        service_pending: h.service_pending.load(Ordering::SeqCst),
        current_thread,
        current_context: cur as usize,
        switch_log: h.switch_log.lock().map(|g| g.clone()).unwrap_or_default(),
        irq,
    }
}
