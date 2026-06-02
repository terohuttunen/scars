//! Self-tests for the synchronous test-harness KHAL.
//!
//! These drive the modeled hardware directly (clock, interrupt
//! controller, alarm) and assert on the [`khal::state`] snapshot — the
//! "check current hardware status" capability the harness exists to
//! provide. They run in idle context like every other unit test under
//! `khal-test`.

use crate::khal;
use scars_khal::{AlarmClockController, InterruptController};

type Hal = khal::HAL;

#[test_case]
fn reset_clears_peripheral_state() {
    // Dirty some state, then confirm reset returns the clean defaults.
    <Hal as InterruptController>::set_interrupt_threshold(5);
    khal::pend_interrupt(7);
    khal::reset();

    let s = khal::state();
    assert_eq!(s.now, 0);
    assert!(s.wakeup.is_none());
    assert_eq!(s.threshold, 0);
    assert_eq!(s.pending, 0);
    assert_eq!(s.wakeups_fired, 0);
    assert!(!s.service_pending);
}

#[test_case]
fn pending_priority_and_enable_are_observable() {
    khal::reset();
    <Hal as InterruptController>::set_interrupt_priority(3, 5);
    <Hal as InterruptController>::enable_interrupt(3);
    khal::pend_interrupt(3);

    let s = khal::state();
    assert_eq!(s.pending & (1 << 3), 1 << 3);
    assert!(s.irq[3].enabled);
    assert_eq!(s.irq[3].priority, 5);
    // An IRQ left untouched stays disabled at priority zero.
    assert!(!s.irq[10].enabled);
    assert_eq!(s.irq[10].priority, 0);
}

#[test_case]
fn threshold_round_trips() {
    khal::reset();
    <Hal as InterruptController>::set_interrupt_threshold(4);
    assert_eq!(<Hal as InterruptController>::get_interrupt_threshold(), 4);
    assert_eq!(khal::state().threshold, 4);
}

#[test_case]
fn clock_advances_monotonically() {
    khal::reset();
    khal::advance(250);
    assert_eq!(khal::state().now, 250);
    khal::advance(100);
    assert_eq!(khal::state().now, 350);
    // advance_to never moves the clock backwards.
    khal::advance_to(100);
    assert_eq!(khal::state().now, 350);
}

#[test_case]
fn armed_wakeup_fires_when_clock_crosses_it() {
    khal::reset();
    <Hal as AlarmClockController>::set_wakeup(Some(1000));
    assert_eq!(khal::state().wakeup, Some(1000));

    khal::advance(500);
    let s = khal::state();
    assert_eq!(s.now, 500);
    assert_eq!(s.wakeups_fired, 0); // deadline not yet reached
    assert_eq!(s.wakeup, Some(1000));

    khal::advance(600); // now 1100 >= 1000
    let s = khal::state();
    assert_eq!(s.now, 1100);
    assert_eq!(s.wakeups_fired, 1);
    assert!(s.wakeup.is_none()); // disarmed after firing
}
