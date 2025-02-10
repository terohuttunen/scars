#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(impl_trait_in_assoc_type)]
use scars::events::wait_events;
use scars::sync::channel::Sender;
use scars::sync::{Condvar, Mutex};
use scars::thread::ThreadRef;
use scars::time::Duration;
use scars::{prelude::*, thread};
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

#[scars::thread(name = "thread0", priority = THREAD0_PRIORITY, stack_size = STACK_SIZE)]
fn thread0(sender: Sender<u32, CAPACITY, CEILING>) -> ! {
    let thread0_ref = unsafe { ThreadRef::current() };
    thread1(sender.clone(), thread0_ref).start();
    wait_events(UNBLOCK_EVENT);
    sender.send(0);
    scars::delay(Duration::from_millis(1000));
    scars_test::test_fail()
}

#[scars::thread(name = "thread1", priority = THREAD1_PRIORITY, stack_size = STACK_SIZE)]
fn thread1(sender: Sender<u32, CAPACITY, CEILING>, thread0_ref: ThreadRef) -> ! {
    let thread1_ref = unsafe { ThreadRef::current() };
    thread2(sender.clone(), thread0_ref, thread1_ref).start();
    wait_events(UNBLOCK_EVENT);
    sender.send(1);
    scars::delay(Duration::from_millis(1000));
    scars_test::test_fail()
}

#[scars::thread(name = "thread2", priority = THREAD2_PRIORITY, stack_size = STACK_SIZE)]
fn thread2(
    sender: Sender<u32, CAPACITY, CEILING>,
    thread0_ref: ThreadRef,
    thread1_ref: ThreadRef,
) -> ! {
    thread0_ref.send_events(UNBLOCK_EVENT);
    thread1_ref.send_events(UNBLOCK_EVENT);
    sender.send(2);
    scars::delay(Duration::from_millis(1000));
    scars_test::test_fail()
}

/// Block a thread waiting for event and release it with an event.
/// Highest priority thread ready to run will be woken up first.
#[test_case]
pub fn block_waiting_event() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    thread0(sender.clone()).start();

    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 0);

    scars_test::test_succeed();
}
