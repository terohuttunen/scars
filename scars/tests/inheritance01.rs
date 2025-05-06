#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(impl_trait_in_assoc_type)]
use core::pin::Pin;
use core::sync::atomic::{AtomicU32, Ordering};
use scars::prelude::*;
use scars::sync::InheritanceLock;
use scars::time::Duration;
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const LOW_PRIORITY: Priority = Priority::thread(3);

// Higher priority thread
const HIGH_PRIORITY: Priority = Priority::thread(5);

// Medium priority thread
const MEDIUM_PRIORITY: Priority = Priority::thread(4);

static LOCK1: InheritanceLock = InheritanceLock::new();
static STATE: AtomicU32 = AtomicU32::new(0);

#[scars::thread(name = "high", priority = HIGH_PRIORITY, stack_size = STACK_SIZE)]
fn high_thread() -> ! {
    let lock1 = Pin::static_ref(&LOCK1);

    let _g = lock1.lock();

    STATE.store(1, Ordering::SeqCst);

    loop {
        scars::delay(Duration::from_secs(1));
    }
}

#[scars::thread(name = "medium", priority = MEDIUM_PRIORITY, stack_size = STACK_SIZE)]
fn medium_thread() -> ! {
    // After high priority thread acquires the lock, writes 1 to STATE, and goes to sleep,
    // medium priority thread can run and write 2 to STATE.
    assert_eq!(STATE.load(Ordering::SeqCst), 1);
    STATE.store(2, Ordering::SeqCst);

    loop {
        scars::delay(Duration::from_secs(1));
    }
}

#[scars::thread(name = "low", priority = LOW_PRIORITY, stack_size = STACK_SIZE)]
fn low_thread() -> ! {
    let pinned_lock1 = Pin::static_ref(&LOCK1);

    let g = pinned_lock1.lock();

    high_thread().start();

    medium_thread().start();

    // High priority thread cannot write 1 to STATE because low priority thread
    // is holding the lock. Medium priority thread cannot run because low priority
    // thread has inherited the high priority.
    assert_eq!(STATE.load(Ordering::SeqCst), 0);

    // Low priority thread releases the lock, allowing high priority thread to run.
    // Low priority thread disinherits the high priority.
    drop(g);

    // High priority thread can write 1 to STATE because the lock has been released.
    // Then medium priority thread can run, and write 2 to STATE.
    // Medium priority thread was able to run, so low priority thread was preempted
    // because its priority was dropped back to its base priority.
    assert_eq!(STATE.load(Ordering::SeqCst), 2);
    STATE.store(3, Ordering::SeqCst);

    loop {
        scars::delay(Duration::from_secs(1));
    }
}

/// Check that a high priority thread trying to acquire an inheritance lock
/// will increase the priority of the low priority thread that is holding the lock.
/// Also tests that medium priority thread cannot preempt a low priority thread that
/// has inherited the high priority.
#[test_case]
pub fn inheritance_lock_priority_increase() {
    low_thread().start();
    assert_eq!(STATE.load(Ordering::SeqCst), 3);
}
