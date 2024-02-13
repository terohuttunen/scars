#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]
use scars::cell::LockedCell;
use scars::prelude::*;
use scars::sync::CeilingLock;
use scars::time::Duration;
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "hal-std"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "hal-std")]
const STACK_SIZE: usize = 16384;

// Lower priority task
const LOW_PRIORITY: u8 = 3;

// Higher priority task
const HIGH_PRIORITY: u8 = 5;

// Medium priority task
const MEDIUM_PRIORITY: u8 = 4;

const CAPACITY: usize = 10;
const CEILING: AnyPriority = any_task_priority(MEDIUM_PRIORITY);

// TODO:
// Nested ceiling lock section prevents preemption by lower priority task
// Ceiling lock prevents preemption by lower priority task
// When task yields while holding a lock, the yield returns to same task
// When task sleeps while holding a lock, no equal or lower priority task can run while the task is sleeping
// Ceiling lock (section and owned) taken by a task at interrupt priorities, blocks interrupts below or at the ceiling.
// Ceiling lock (section and owned) taken by a task at interrupt priorities, do not block higher priority interrupts

/// Ceiling lock section prevents preemption by lower priority task
#[test_case]
pub fn ceiling_lock_section_preempt() {
    let (sender0, receiver) = make_channel!(u32, CAPACITY, any_task_priority(HIGH_PRIORITY));
    let protected_data: LockedCell<usize, CeilingLock<CEILING>> = LockedCell::new(0);
    let low = make_task!("low", LOW_PRIORITY, STACK_SIZE);

    low.start(move || {
        let medium = make_task!("medium", MEDIUM_PRIORITY, STACK_SIZE);
        let high = make_task!("high", HIGH_PRIORITY, STACK_SIZE);
        let medium_sender = sender0.clone();
        let high_sender = sender0.clone();

        // Low priority task raises its priority with a ceiling lock section
        CeilingLock::with(|ckey| {
            protected_data.set(ckey, 1);
            // Medium priority task cannot start because of the ceiling lock
            medium.start(move || {
                medium_sender.send(1);
                loop {
                    scars::delay(Duration::from_secs(1));
                }
            });
            // High priority task can start because it is above the ceiling
            high.start(move || {
                high_sender.send(2);
                loop {
                    scars::delay(Duration::from_secs(1));
                }
            });
        });
        // Medium priority task can run now, and then low priority continues
        sender0.send(0);
        loop {
            scars::delay(Duration::from_secs(1));
        }
    });
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 0);

    scars_test::test_succeed();
}
