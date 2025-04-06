#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(impl_trait_in_assoc_type)]
use scars::prelude::*;
use scars::sync::CeilingLock;
use scars::sync::channel::Sender;
use scars::time::Duration;
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024 * 4;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const LOW_PRIORITY: Priority = Priority::thread(3);

// Higher priority thread
const HIGH_PRIORITY: Priority = Priority::thread(5);

// Medium priority thread
const MEDIUM_PRIORITY: Priority = Priority::thread(4);

const CAPACITY: usize = 10;
const CEILING: Priority = MEDIUM_PRIORITY;

/// Ceiling lock prevents preemption by lower priority thread
#[test_case]
pub fn ceiling_lock_owned_preempt() {
    let (sender0, receiver) = make_channel!(u32, CAPACITY, HIGH_PRIORITY);

    low_thread(sender0.clone(), sender0.clone(), sender0.clone()).start();

    assert_eq!(receiver.recv(), 3);
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 0);
}

#[scars::thread(name = "low", priority = LOW_PRIORITY, stack_size = STACK_SIZE)]
fn low_thread(
    sender0: Sender<u32, CAPACITY, HIGH_PRIORITY>,
    medium_sender: Sender<u32, CAPACITY, HIGH_PRIORITY>,
    high_sender: Sender<u32, CAPACITY, HIGH_PRIORITY>,
) -> ! {
    let medium_sender = medium_sender.clone();
    let high_sender = high_sender.clone();
    let lock: CeilingLock<CEILING> = CeilingLock::new();

    // Low priority thread raises its priority with a ceiling lock
    let pinned = core::pin::pin!(lock);
    let _guard = pinned.as_ref().lock();
    // Medium priority thread cannot run because of the ceiling lock
    medium_thread(medium_sender).start();
    // High priority thread can run because it is above the ceiling
    high_thread(high_sender).start();
    sender0.send(2);
    unsafe { pinned.as_ref().unlock() };
    // Medium priority thread can run now, and then low priority continues
    sender0.send(0);
    loop {
        scars::delay(Duration::from_secs(1));
    }
}

#[scars::thread(name = "medium", priority = MEDIUM_PRIORITY, stack_size = STACK_SIZE)]
fn medium_thread(medium_sender: Sender<u32, CAPACITY, HIGH_PRIORITY>) -> ! {
    medium_sender.send(1);
    loop {
        scars::delay(Duration::from_secs(1));
    }
}

#[scars::thread(name = "high", priority = HIGH_PRIORITY, stack_size = STACK_SIZE)]
fn high_thread(high_sender: Sender<u32, CAPACITY, HIGH_PRIORITY>) -> ! {
    high_sender.send(3);
    loop {
        scars::delay(Duration::from_secs(1));
    }
}
