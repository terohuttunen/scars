#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]
use scars::events::{wait_events, REQUIRE_ALL_EVENTS};
use scars::prelude::*;
use scars::sync::{Condvar, Mutex};
use scars::time::Duration;
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024 * 2;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const THREAD0_PRIORITY: u8 = 3;

// Higher priority thread
const THREAD1_PRIORITY: u8 = 5;

// Medium priority thread
const THREAD2_PRIORITY: u8 = 4;

const CAPACITY: usize = 14;
const CEILING: AnyPriority = any_thread_priority(THREAD1_PRIORITY);

const UNBLOCK_EVENT1: u32 = 1u32;
const UNBLOCK_EVENT2: u32 = 2u32;

/// Block a thread waiting for event and release it with an event.
/// Highest priority thread ready to run will be woken up first.
#[test_case]
pub fn block_waiting_event() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    let thread0 = make_thread!("thread0", THREAD0_PRIORITY, STACK_SIZE);
    let thread0_ref = thread0.as_ref();
    let sender0 = sender.clone();
    thread0.start(move || {
        let thread1 = make_thread!("thread1", THREAD1_PRIORITY, STACK_SIZE);
        let thread1_ref = thread1.as_ref();
        let sender1 = sender0.clone();
        thread1.start(move || {
            // Medium priority thread created last
            let thread2 = make_thread!("thread2", THREAD2_PRIORITY, STACK_SIZE);
            let sender2 = sender1.clone();
            thread2.start(move || {
                thread0_ref.send_events(UNBLOCK_EVENT1);
                thread1_ref.send_events(UNBLOCK_EVENT1);
                sender2.send(2);
                thread0_ref.send_events(UNBLOCK_EVENT2);
                thread1_ref.send_events(UNBLOCK_EVENT2);
                sender2.send(3);
                scars::delay(Duration::from_millis(1000));
                scars_test::test_fail()
            });

            wait_events(UNBLOCK_EVENT1 | UNBLOCK_EVENT2 | REQUIRE_ALL_EVENTS);
            sender1.send(1);
            scars::delay(Duration::from_millis(1000));
            scars_test::test_fail()
        });

        wait_events(UNBLOCK_EVENT1 | UNBLOCK_EVENT2 | REQUIRE_ALL_EVENTS);
        sender0.send(0);
        scars::delay(Duration::from_millis(1000));
        scars_test::test_fail()
    });

    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 3);
    assert_eq!(receiver.recv(), 0);

    scars_test::test_succeed();
}