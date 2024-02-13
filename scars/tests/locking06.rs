#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]
use core::sync::atomic::{AtomicBool, Ordering};
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

// Medium priority task
const MEDIUM_PRIORITY: u8 = 4;

const CAPACITY: usize = 10;
const CEILING: AnyPriority = any_task_priority(MEDIUM_PRIORITY);

static IDLE_HAS_RUN: AtomicBool = AtomicBool::new(false);

#[scars::idle_task_hook]
fn idle() {
    IDLE_HAS_RUN.store(true, Ordering::SeqCst);
}

/// Is possible for a task to sleep and hold the lock
#[test_case]
pub fn ceiling_lock_owned_yield() {
    let (sender0, receiver) =
        make_channel!(u32, CAPACITY, Priority::any_task_priority(MEDIUM_PRIORITY));

    let low = make_task!("low", LOW_PRIORITY, STACK_SIZE);

    low.start(move || {
        let medium = make_task!("medium", MEDIUM_PRIORITY, STACK_SIZE);
        let medium_sender = sender0.clone();
        let lock: CeilingLock<CEILING> = CeilingLock::new();

        // Low priority task raises its priority with a ceiling lock
        lock.lock();
        // Medium priority task cannot start because of the ceiling lock
        medium.start(move || {
            medium_sender.send(1);
            loop {
                scars::delay(Duration::from_secs(1));
            }
        });
        // Low priority task goes to sleep, but idle task will
        // execute instead of medium priority task because low priority
        // task is holding the lock while sleeping.
        assert!(!IDLE_HAS_RUN.load(Ordering::SeqCst));
        scars::delay(Duration::from_millis(50));
        assert!(IDLE_HAS_RUN.load(Ordering::SeqCst));
        sender0.send(2);
        lock.unlock();
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
