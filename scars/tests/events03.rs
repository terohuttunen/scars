#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]
use scars::events::wait_events_until;
use scars::prelude::*;
use scars::sync::{Condvar, Mutex};
use scars::thread_suspend;
use scars::time::Duration;
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024 * 2;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const THREAD0_PRIORITY: u8 = 3;

const CAPACITY: usize = 14;
const CEILING: AnyPriority = any_thread_priority(THREAD0_PRIORITY);

const UNBLOCK_EVENT: u32 = 1u32;

/// Block a thread waiting for event and let it timeout
#[test_case]
pub fn block_waiting_event() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    let thread0 = make_thread!("thread0", THREAD0_PRIORITY, STACK_SIZE);
    let thread0_ref = thread0.as_ref();
    let sender0 = sender.clone();
    thread0.start(move || {
        let deadline = scars::time::Instant::now() + Duration::from_millis(10);
        let wait_result = wait_events_until(UNBLOCK_EVENT, Some(deadline));
        assert!(wait_result.is_err());
        sender0.send(0);
        thread_suspend(None);
    });

    assert_eq!(receiver.recv(), 0);

    scars_test::test_succeed();
}
