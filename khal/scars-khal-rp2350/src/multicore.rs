//! Core 1 launch sequence (RP2350 boot-ROM hand-off).
//!
//! Core 0's `HardwareAbstractionLayer::init` calls [`launch_core1`]
//! after the on-die clocks and core 0's NVIC are up. The boot ROM
//! waits on core 1 for a 6-word handshake (`0, 0, 1, VTOR, SP, entry`)
//! over the SIO inter-core FIFO; once received it sets core 1's
//! VTOR/MSP and branches to `entry` — which lands in [`core1_entry`]
//! here. Core 1 then enables its own NVIC IRQs, posts
//! [`CORE1_ALIVE_SENTINEL`] back to core 0 so `launch_core1` can
//! return, and dives into [`scars_khal::start_kernel`] which
//! installs core 1's scheduler and idle thread (the kernel reads
//! `current_core_id()` to skip `init_hal` on this side).
//!
//! The FIFO helpers here are separate from `ipi::ipi_push` on
//! purpose: boot needs `sev`/`wfe` synchronisation with the boot
//! ROM, and the launch handshake is one-shot — IPI is hot path and
//! optimised for it.

use scars_arch_cortex_m::{init_pendsv_priority, nvic};
use scars_khal::InterruptController;

use crate::{RP2350, pac};

unsafe extern "C" {
    /// Linker-provided top of core 1's MSP region. The region's size
    /// is `_core1_isr_stack_size` (linker `PROVIDE`, default 1024 B)
    /// and it sits at the very top of RAM, immediately above core 0's
    /// cortex-m-rt boot/ISR stack. See `default.lds` for the layout.
    /// We pass `&_core1_stack_top as u32` as the SP word in the
    /// boot-ROM hand-off; core 1 then uses this region for every
    /// exception entry for the lifetime of the program.
    static _core1_stack_top: u8;
}

/// Sentinel that core 1 posts back to core 0 once core 1's
/// `Scheduler::start_on(1)` has finished publishing `SCHEDULERS[1]`
/// and setting `SCHEDULER_INITIALIZED[1]`. Lets [`launch_core1`]
/// block until cross-core ops targeting core 1 are safe — without
/// this synchronisation point, core 0's first
/// `Scheduler::schedule_deferred_operation_on(1, ...)` after init
/// can race against core 1's scheduler init and read uninitialised
/// memory from `SCHEDULERS[1]`.
const CORE1_ALIVE_SENTINEL: u32 = 0xC0DE_A11E;

/// Posted by RP2350's `CoreController::start_first_thread` when
/// invoked on core 1 — i.e., after `Scheduler::start_on(1)` has
/// completed scheduler publication.
pub(crate) fn signal_core1_alive() {
    fifo_push_blocking(CORE1_ALIVE_SENTINEL);
}

#[inline]
fn fifo_st_rdy() -> bool {
    let sio = unsafe { &*pac::SIO::ptr() };
    sio.fifo_st().read().rdy().bit_is_set()
}

#[inline]
fn fifo_st_vld() -> bool {
    let sio = unsafe { &*pac::SIO::ptr() };
    sio.fifo_st().read().vld().bit_is_set()
}

fn fifo_push_blocking(value: u32) {
    let sio = unsafe { &*pac::SIO::ptr() };
    while !fifo_st_rdy() {
        cortex_m::asm::nop();
    }
    sio.fifo_wr().write(|w| unsafe { w.bits(value) });
    cortex_m::asm::sev();
}

fn fifo_pop_blocking() -> u32 {
    let sio = unsafe { &*pac::SIO::ptr() };
    while !fifo_st_vld() {
        cortex_m::asm::nop();
    }
    sio.fifo_rd().read().bits()
}

fn fifo_drain() {
    let sio = unsafe { &*pac::SIO::ptr() };
    while fifo_st_vld() {
        let _ = sio.fifo_rd().read().bits();
    }
}

/// Reset core 1 via PSM so it re-enters the bootrom and parks in the
/// launch-wait state. Mirrors pico-sdk's `multicore_reset_core1`:
/// set PSM.FRCE_OFF.proc1, wait for the readback, then clear it.
/// Without this, core 1 may already have exited the bootrom (or be
/// in some indeterminate state from a previous run) and the launch
/// handshake will hang forever waiting for a peer that isn't there.
fn reset_core1() {
    let psm = unsafe { &*pac::PSM::ptr() };
    psm.frce_off().modify(|_, w| w.proc1().set_bit());
    while !psm.frce_off().read().proc1().bit_is_set() {
        cortex_m::asm::nop();
    }
    psm.frce_off().modify(|_, w| w.proc1().clear_bit());
    // Drain any junk in the FIFO that may have been left over from a
    // previous launch attempt before we restart the handshake.
    fifo_drain();
}

/// Run the RP2350 boot-ROM hand-off for core 1. Mirrors pico-sdk's
/// `multicore_launch_core1_raw`: a 6-word sequence (0, 0, 1, VTOR,
/// SP, entry) is sent over the inter-core FIFO and each word must be
/// echoed back before advancing; an echo mismatch restarts the
/// sequence. After the entry word is echoed, this function blocks
/// until [`core1_entry`] posts [`CORE1_ALIVE_SENTINEL`].
pub(crate) fn launch_core1() {
    // Force core 1 back into the bootrom launch-wait state before
    // starting the handshake. The application image's reset path
    // doesn't deterministically leave core 1 in the right state on
    // its own (verified on Pico 2 — without this, the handshake
    // hangs because nothing is listening on core 1's side).
    reset_core1();
    // Core 0's VTOR — both cores share the same vector table layout,
    // we just need core 1 to load the same base into its own SCB.
    let vtor = unsafe { (*cortex_m::peripheral::SCB::PTR).vtor.read() };
    // Top-of-stack from the linker (top of RAM by default). The
    // region's size is `_core1_isr_stack_size`, configured in
    // `default.lds`.
    let stack_top = &raw const _core1_stack_top as u32;
    // Thumb bit set on entry pointer.
    let entry = (core1_entry as u32) | 1;

    let cmd_sequence = [0u32, 0, 1, vtor, stack_top, entry];
    let mut seq = 0usize;
    while seq < cmd_sequence.len() {
        let cmd = cmd_sequence[seq];
        if cmd == 0 {
            // Drain any stale RX before the resync command, then SEV
            // to nudge core 1 out of its WFE in the boot ROM.
            fifo_drain();
            cortex_m::asm::sev();
        }
        fifo_push_blocking(cmd);
        let response = fifo_pop_blocking();
        seq = if cmd == response { seq + 1 } else { 0 };
    }

    // Block until core 1's scheduler is fully published (sentinel is
    // posted from RP2350's `CoreController::start_first_thread`
    // after `SCHEDULER_INITIALIZED[1]` is set).
    loop {
        if fifo_pop_blocking() == CORE1_ALIVE_SENTINEL {
            break;
        }
    }
    // FIFO_RD is a Device-mapped MMIO read, which is not ordered
    // against normal memory by the Cortex-M33 memory model. A `dmb`
    // here forces core 0 to observe the normal-memory stores core 1
    // performed before its FIFO_WR — most importantly
    // `SCHEDULERS[1] = Scheduler::new(...)` and the Release on
    // `SCHEDULER_INITIALIZED[1]`. Without this, core 0's first
    // cross-core dispatch can still hit stale memory.
    cortex_m::asm::dmb();
}

/// Core 1's Rust entry point. Reached after the boot ROM has loaded
/// the launch sequence's VTOR/SP/entry words. Sets up the core-local
/// kernel registers, enables core 1's NVIC IRQs, then enters
/// [`scars_khal::start_kernel`] — same symbol the default core uses;
/// the kernel branches on `current_core_id()` to decide whether to
/// run `init_hal` (it doesn't, on this side).
///
/// We deliberately do NOT post `CORE1_ALIVE_SENTINEL` here even
/// though core 0 is blocking on it. Sending it now would let core 0
/// proceed before `Scheduler::start_on(1)` has published
/// `SCHEDULERS[1]`, causing a race on the first cross-core dispatch.
/// The sentinel is posted from inside RP2350's
/// `CoreController::start_first_thread`, which runs after
/// `SCHEDULER_INITIALIZED[1]` is set.
unsafe extern "C" fn core1_entry() -> ! {
    let cortex_m::Peripherals { mut SCB, .. } = unsafe { cortex_m::Peripherals::steal() };
    init_pendsv_priority(&mut SCB);

    // Enable IPI handler + this core's alarm in core 1's own NVIC.
    // Each core has its own NVIC enable state, so this must be set
    // here even though core 0 did the analogous setup in `init`.
    nvic::enable(pac::Interrupt::SIO_IRQ_FIFO as u16);
    nvic::enable(pac::Interrupt::TIMER0_IRQ_1 as u16);
    <RP2350 as InterruptController>::set_interrupt_priority(pac::Interrupt::SIO_IRQ_FIFO as u16, 0);
    <RP2350 as InterruptController>::set_interrupt_priority(pac::Interrupt::TIMER0_IRQ_1 as u16, 0);

    unsafe { scars_khal::start_kernel() }
}
