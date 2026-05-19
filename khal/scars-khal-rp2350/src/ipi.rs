//! Cross-core inter-processor interrupt over the SIO FIFO.
//!
//! `pend_service_call_on(other)` in `context::CoreController` calls
//! [`ipi_push`] to wake the sibling core. The hardware delivers the
//! write as a `SIO_IRQ_FIFO` on the other core, where the trampoline
//! at the bottom of this file drains the FIFO and pends a local
//! PendSV; the kernel's service-call dispatcher then runs and
//! processes whatever the producer pushed into the target's
//! `deferred_work_queue` (the FIFO value itself carries no payload —
//! the act of waking is the signal).
//!
//! Boot-time core-1 launch uses different FIFO helpers in
//! `multicore`; those wait/sev around each push and explicitly drain.
//! The IPI path here is tuned for hot kernel use: a DSB to publish
//! producer-side state before the write, a tight spin while RDY=0,
//! and nothing else.

use crate::pac;

/// Sentinel pushed by [`ipi_push`] and discarded by
/// [`_scars_rp2350_sio_fifo_irq`]. The value carries no information —
/// the act of waking the target core is the signal.
pub(crate) const IPI_SENTINEL: u32 = 0x5CA5_5191;

/// Producer-side IPI push. Caller has already published whatever
/// kernel state the receiver should observe (typically a
/// `deferred_work_queue` push with its AcqRel release fence). DSB
/// here makes that release globally visible before the FIFO write so
/// the target's IRQ entry doesn't outrun the producer's data.
pub(crate) fn ipi_push(value: u32) {
    let sio = unsafe { &*pac::SIO::ptr() };
    cortex_m::asm::dsb();
    while !sio.fifo_st().read().rdy().bit_is_set() {
        core::hint::spin_loop();
    }
    sio.fifo_wr().write(|w| unsafe { w.bits(value) });
}

/// Rust half of the SIO FIFO IRQ. Drains the inbound FIFO, clears
/// any error latches, and pends the local PendSV so the kernel's
/// service-call dispatcher runs and drains its
/// `deferred_work_queue`. The actual cross-core work (start /
/// resume / suspend) is encoded in that queue, not in the FIFO
/// bytes themselves.
#[unsafe(no_mangle)]
extern "C" fn _scars_rp2350_sio_fifo_irq() {
    let sio = unsafe { &*pac::SIO::ptr() };
    while sio.fifo_st().read().vld().bit_is_set() {
        let _ = sio.fifo_rd().read().bits();
    }
    // WOF (bit 4) and ROE (bit 3) are write-1-to-clear; clear both so
    // a transient overflow or read-empty during testing doesn't latch.
    sio.fifo_st()
        .write(|w| unsafe { w.bits((1 << 4) | (1 << 3)) });
    scars_arch_cortex_m::pend_service_call();
}

/// SIO_IRQ_FIFO — naked trampoline. Calls the Rust handler then
/// returns through the normal IRQ exit path. No `b _switch_context`:
/// the context switch (if any) happens later when PendSV fires.
#[unsafe(naked)]
#[unsafe(export_name = "SIO_IRQ_FIFO")]
#[unsafe(link_section = ".SIO_IRQ_FIFO.user")]
pub unsafe extern "C" fn sio_irq_fifo() {
    core::arch::naked_asm!(
        "push   {{r4, lr}}", // r4 for alignment
        "bl     _scars_rp2350_sio_fifo_irq",
        "pop    {{r4, lr}}",
        "bx     lr",
    );
}
