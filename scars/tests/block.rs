#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]
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

/// Block two threads with different priorities and verify that the
/// higher priority thread is started first when the threads are notified.
#[test_case]
pub fn block_unblock_thread() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);
    static LOCK: Mutex<bool, CEILING> = Mutex::new(false);
    static CVAR: Condvar<CEILING> = Condvar::new();

    let thread0 = make_thread!("thread0", THREAD0_PRIORITY, STACK_SIZE);
    let sender0 = sender.clone();
    thread0.start(move || {
        let thread1 = make_thread!("thread1", THREAD1_PRIORITY, STACK_SIZE);
        let sender1 = sender0.clone();
        thread1.start(move || {
            // Medium priority thread created last
            let thread2 = make_thread!("thread2", THREAD2_PRIORITY, STACK_SIZE);
            let sender2 = sender1.clone();
            thread2.start(move || {
                let guarded_started = LOCK.lock();
                CVAR.wait_while(guarded_started, |started| !*started);
                sender2.send(2);
                scars::delay(Duration::from_millis(1000));
                scars_test::test_fail()
            });

            let guarded_started = LOCK.lock();
            CVAR.wait_while(guarded_started, |started| !*started);
            sender1.send(1);
            scars::delay(Duration::from_millis(1000));
            scars_test::test_fail()
        });

        let guarded_started = LOCK.lock();
        CVAR.wait_while(guarded_started, |started| !*started);
        sender0.send(0);
        scars::delay(Duration::from_millis(1000));
        scars_test::test_fail()
    });

    // Release the barrier flag holding the treads and notify threads
    *LOCK.lock() = true;
    CVAR.notify_all();

    // Higher priority thread 1 is woken up first.
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 0);

    scars_test::test_succeed();
}
