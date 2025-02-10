#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(impl_trait_in_assoc_type)]
use scars::cell::LockedCell;
use scars::prelude::*;
use scars::sync::CeilingLock;
use scars::sync::channel::Sender;
use scars::time::Duration;
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const LOW_PRIORITY: Priority = Priority::thread(3);

// Medium priority thread
const MEDIUM_PRIORITY: Priority = Priority::thread(4);

const CAPACITY: usize = 10;
const CEILING: Priority = MEDIUM_PRIORITY;

#[scars::thread(name = "low", priority = LOW_PRIORITY, stack_size = STACK_SIZE)]
fn low_thread(
    sender0: Sender<u32, CAPACITY, CEILING>,
    medium_sender: Sender<u32, CAPACITY, CEILING>,
    protected_data: LockedCell<usize, CeilingLock<CEILING>>,
) -> ! {
    let medium_sender = medium_sender.clone();
    // Low priority thread raises its priority with a ceiling lock section
    CeilingLock::with(|ckey| {
        protected_data.set(ckey, 1);
        // Medium priority thread cannot start because of the ceiling lock
        medium_thread(medium_sender).start();
        sender0.send(2);
    });
    // Medium priority thread can run now, and then low priority continues
    sender0.send(0);
    loop {
        scars::delay(Duration::from_secs(1));
    }
}

#[scars::thread(name = "medium", priority = MEDIUM_PRIORITY, stack_size = STACK_SIZE)]
fn medium_thread(sender: Sender<u32, CAPACITY, CEILING>) -> ! {
    sender.send(1);
    loop {
        scars::delay(Duration::from_secs(1));
    }
}

/// Ceiling lock section prevents preemption by a thread at ceiling priority,
/// and when the lock section ends, the highest priority thread will run.
#[test_case]
pub fn ceiling_lock_section_yield() {
    let (sender0, receiver) = make_channel!(u32, CAPACITY, MEDIUM_PRIORITY);
    let protected_data: LockedCell<usize, CeilingLock<CEILING>> = LockedCell::new(0);

    low_thread(sender0.clone(), sender0.clone(), protected_data).start();

    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 0);

    scars_test::test_succeed();
}
