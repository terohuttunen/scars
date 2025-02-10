#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(impl_trait_in_assoc_type)]
use scars::events::wait_events_until;
use scars::prelude::*;
use scars::sync::{Condvar, Mutex, channel::Sender};
use scars::thread_suspend;
use scars::time::Duration;
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024 * 2;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const THREAD0_PRIORITY: Priority = Priority::thread(3);

const CAPACITY: usize = 14;
const CEILING: Priority = THREAD0_PRIORITY;

const UNBLOCK_EVENT: u32 = 1u32;

#[scars::thread(name = "thread0", priority = THREAD0_PRIORITY, stack_size = STACK_SIZE)]
fn thread0(sender: Sender<u32, CAPACITY, CEILING>) -> ! {
    let deadline = scars::time::Instant::now() + Duration::from_millis(10);
    let wait_result = wait_events_until(UNBLOCK_EVENT, Some(deadline));
    assert!(wait_result.is_err());
    sender.send(0);
    thread_suspend(None);
}

/// Block a thread waiting for event and let it timeout
#[test_case]
pub fn block_waiting_event() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    thread0(sender).start();

    assert_eq!(receiver.recv(), 0);

    scars_test::test_succeed();
}
