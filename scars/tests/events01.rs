#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]
use scars::events::wait_events;
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
const THREAD0_PRIORITY: Priority = Priority::thread(3);

// Higher priority thread
const THREAD1_PRIORITY: Priority = Priority::thread(5);

// Medium priority thread
const THREAD2_PRIORITY: Priority = Priority::thread(4);

const CAPACITY: usize = 14;
const CEILING: Priority = THREAD0_PRIORITY.max(THREAD1_PRIORITY).max(THREAD2_PRIORITY);

const UNBLOCK_EVENT: u32 = 1u32;

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
                thread0_ref.send_events(UNBLOCK_EVENT);
                thread1_ref.send_events(UNBLOCK_EVENT);
                sender2.send(2);
                scars::delay(Duration::from_millis(1000));
                scars_test::test_fail()
            });

            wait_events(UNBLOCK_EVENT);
            sender1.send(1);
            scars::delay(Duration::from_millis(1000));
            scars_test::test_fail()
        });

        wait_events(UNBLOCK_EVENT);
        sender0.send(0);
        scars::delay(Duration::from_millis(1000));
        scars_test::test_fail()
    });

    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 0);

    scars_test::test_succeed();
}
