#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(impl_trait_in_assoc_type)]
use core::sync::atomic::{AtomicBool, Ordering};
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

static IDLE_HAS_RUN: AtomicBool = AtomicBool::new(false);

#[scars::idle_thread_hook]
fn idle() {
    IDLE_HAS_RUN.store(true, Ordering::SeqCst);
}

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
        // Low priority thread goes to sleep, but idle thread will
        // execute instead of medium priority thread because low
        // priority thread is holding the lock while sleeping.
        assert!(!IDLE_HAS_RUN.load(Ordering::SeqCst));
        scars::delay(Duration::from_millis(50));
        assert!(IDLE_HAS_RUN.load(Ordering::SeqCst));
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

/// Is possible for a thread to sleep and hold the lock
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
