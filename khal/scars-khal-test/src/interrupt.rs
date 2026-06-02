use crate::{TestHal, hal};
use core::sync::atomic::Ordering;
use scars_khal::*;

pub const MAX_INTERRUPT_PRIORITY: u8 =
    <TestHal as InterruptController>::MAX_INTERRUPT_PRIORITY as u8;
pub const MAX_INTERRUPT: usize = <TestHal as InterruptController>::MAX_INTERRUPT_NUMBER;

pub struct InterruptClaim {
    interrupt_number: u16,
    restore_threshold: u8,
}

impl GetInterruptNumber for InterruptClaim {
    fn get_interrupt_number(&self) -> u16 {
        self.interrupt_number
    }
}

/// Highest-priority enabled pending IRQ above the current threshold.
///
/// Identical selection logic to the simulator's controller, minus the
/// signal delivery: the harness never dispatches on its own, the test
/// drives delivery through [`crate::pump`].
pub(crate) fn find_runnable() -> Option<u16> {
    let ic = hal();
    let pending = ic.pending.load(Ordering::SeqCst);
    let threshold = ic.threshold.load(Ordering::SeqCst);
    let no_mask = threshold >= MAX_INTERRUPT_PRIORITY;
    let mut best: Option<(u16, u8)> = None;
    let mut bits = pending;
    while bits != 0 {
        let i = bits.trailing_zeros() as usize;
        bits &= !(1u32 << i);
        if i > MAX_INTERRUPT {
            break;
        }
        if !ic.enable[i].load(Ordering::SeqCst) {
            continue;
        }
        let prio = ic.priority[i].load(Ordering::SeqCst);
        if !no_mask && prio <= threshold {
            continue;
        }
        if best.is_none_or(|(_, p)| prio > p) {
            best = Some((i as u16, prio));
        }
    }
    best.map(|(i, _)| i)
}

/// Pend virtual interrupt `n`.
///
/// Records the pending bit only. Unlike the simulator there is no
/// asynchronous delivery: the test makes the kernel observe the IRQ by
/// calling [`crate::pump`].
pub fn pend_interrupt(n: u16) {
    hal().pending.fetch_or(1u32 << (n as u32), Ordering::SeqCst);
}

impl InterruptController for TestHal {
    const MAX_INTERRUPT_PRIORITY: usize = 7;
    const MAX_INTERRUPT_NUMBER: usize = crate::NUM_IRQ - 1;
    type InterruptClaim = InterruptClaim;

    fn get_interrupt_priority(interrupt_number: u16) -> u8 {
        hal().priority[interrupt_number as usize].load(Ordering::SeqCst)
    }

    fn set_interrupt_priority(interrupt_number: u16, prio: u8) -> u8 {
        hal().priority[interrupt_number as usize].swap(prio, Ordering::SeqCst)
    }

    #[inline(always)]
    fn get_interrupt_threshold() -> u8 {
        hal().threshold.load(Ordering::SeqCst)
    }

    #[inline(always)]
    fn set_interrupt_threshold(threshold: u8) {
        hal().threshold.store(threshold, Ordering::SeqCst);
    }

    fn claim_interrupt() -> InterruptClaim {
        let interrupt_number = find_runnable().expect("claim_interrupt with no runnable IRQ");
        let ic = hal();
        ic.pending
            .fetch_and(!(1u32 << (interrupt_number as u32)), Ordering::SeqCst);
        let interrupt_prio = ic.priority[interrupt_number as usize].load(Ordering::SeqCst);
        let restore_threshold = ic.threshold.swap(interrupt_prio, Ordering::SeqCst);
        InterruptClaim {
            interrupt_number,
            restore_threshold,
        }
    }

    fn complete_interrupt(claim: InterruptClaim) {
        hal()
            .threshold
            .store(claim.restore_threshold, Ordering::SeqCst);
    }

    fn enable_interrupt(interrupt_number: u16) {
        hal().enable[interrupt_number as usize].store(true, Ordering::SeqCst);
    }

    fn disable_interrupt(interrupt_number: u16) {
        hal().enable[interrupt_number as usize].store(false, Ordering::SeqCst);
    }

    #[inline(always)]
    fn interrupt_status() -> bool {
        hal().interrupts_enabled.load(Ordering::SeqCst)
    }

    fn acquire() -> bool {
        hal().interrupts_enabled.swap(false, Ordering::SeqCst)
    }

    fn restore(restore_state: bool) {
        if restore_state {
            hal().interrupts_enabled.store(true, Ordering::SeqCst);
        }
    }
}
