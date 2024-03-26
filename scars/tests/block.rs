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
const STACK_SIZE: usize = 1024*2;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority task
const TASK0_PRIORITY: u8 = 3;

// Higher priority task
const TASK1_PRIORITY: u8 = 5;

// Medium priority task
const TASK2_PRIORITY: u8 = 4;

const CAPACITY: usize = 14;
const CEILING: AnyPriority = any_task_priority(TASK1_PRIORITY);

/// Block two tasks with different priorities and verify that the
/// higher priority task is started first when the tasks are notified.
#[test_case]
pub fn block_unblock_task() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);
    static LOCK: Mutex<bool, CEILING> = Mutex::new(false);
    static CVAR: Condvar<CEILING> = Condvar::new();

    let task0 = make_task!("task0", TASK0_PRIORITY, STACK_SIZE);
    let sender0 = sender.clone();
    task0.start(move || {
        let task1 = make_task!("task1", TASK1_PRIORITY, STACK_SIZE);
        let sender1 = sender0.clone();
        task1.start(move || {
            // Medium priority task created last
            let task2 = make_task!("task2", TASK2_PRIORITY, STACK_SIZE);
            let sender2 = sender1.clone();
            task2.start(move || {
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

    // Release the barrier flag holding the tasks and notify tasks
    *LOCK.lock() = true;
    CVAR.notify_all();

    // Higher priority task 1 is woken up first.
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 0);

    scars_test::test_succeed();
}
