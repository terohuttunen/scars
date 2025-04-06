#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(impl_trait_in_assoc_type)]
use scars::prelude::*;
use scars::sync::channel::Sender;
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

static LOCK: Mutex<bool, CEILING> = Mutex::new(false);
static CVAR: Condvar<CEILING> = Condvar::new();

#[scars::thread(name = "thread0", priority = THREAD0_PRIORITY, stack_size = STACK_SIZE)]
fn thread0(sender: Sender<u32, CAPACITY, CEILING>) -> ! {
    let sender0 = sender.clone();
    thread1(sender0).start();
    let guarded_started = LOCK.lock();
    CVAR.wait_while(guarded_started, |started| !*started);
    sender.send(0);
    scars::delay(Duration::from_millis(1000));
    scars_test::test_fail()
}

#[scars::thread(name = "thread1", priority = THREAD1_PRIORITY, stack_size = STACK_SIZE)]
fn thread1(sender: Sender<u32, CAPACITY, CEILING>) -> ! {
    let sender1 = sender.clone();
    thread2(sender1).start();
    let guarded_started = LOCK.lock();
    CVAR.wait_while(guarded_started, |started| !*started);
    sender.send(1);
    scars::delay(Duration::from_millis(1000));
    scars_test::test_fail()
}

#[scars::thread(name = "thread2", priority = THREAD2_PRIORITY, stack_size = STACK_SIZE)]
fn thread2(sender: Sender<u32, CAPACITY, CEILING>) -> ! {
    let guarded_started = LOCK.lock();
    CVAR.wait_while(guarded_started, |started| !*started);
    sender.send(2);
    scars::delay(Duration::from_millis(1000));
    scars_test::test_fail()
}

/// Block two threads with different priorities and verify that the
/// higher priority thread is started first when the threads are notified.
#[test_case]
pub fn block_unblock_thread() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    thread0(sender).start();

    // Release the barrier flag holding the treads and notify threads
    *LOCK.lock() = true;
    CVAR.notify_all();

    // Higher priority thread 1 is woken up first.
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 0);
}
