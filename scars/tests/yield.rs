#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]
use scars::prelude::*;
use scars::sync::channel::Sender;
use scars::time::Duration;
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "hal-std"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "hal-std")]
const STACK_SIZE: usize = 16384;

// Lower priority task
const TASK0_PRIORITY: u8 = 3;

// Higher priority FIFO tasks
const TASK1_PRIORITY: u8 = 5;

const CAPACITY: usize = 14;
const CEILING: AnyPriority = any_task_priority(TASK1_PRIORITY);

/// When task yields it never switches to lower priority task, and will
/// alternate with same priority tasks in FIFO order.
#[test_case]
pub fn yield_fifo() {
    fn send_numbers_and_sleep(
        sender: &mut Sender<u32, CAPACITY, CEILING>,
        number: u32,
        count: usize,
    ) {
        for _i in 0..count {
            sender.send(number);
            scars::task_yield();
        }

        scars::delay(Duration::from_millis(1000));
    }

    let task0 = make_task!("task0", TASK0_PRIORITY, STACK_SIZE);
    let (mut sender0, receiver) = make_channel!(u32, CAPACITY, CEILING);
    task0.start(move || {
        let task1 = make_task!("task1", TASK1_PRIORITY, STACK_SIZE);
        let mut sender1 = sender0.clone();
        task1.start(move || {
            let task2 = make_task!("task2", TASK1_PRIORITY, STACK_SIZE);
            let mut sender2 = sender1.clone();
            task2.start(move || {
                let task3 = make_task!("task3", TASK1_PRIORITY, STACK_SIZE);
                let mut sender3 = sender2.clone();
                task3.start(move || {
                    send_numbers_and_sleep(&mut sender3, 3, 3);
                    scars_test::test_fail();
                });

                send_numbers_and_sleep(&mut sender2, 2, 4);
                scars_test::test_fail();
            });

            send_numbers_and_sleep(&mut sender1, 1, 4);
            scars_test::test_fail();
        });
        // Higher priority tasks will block execution of this task after task1 is started
        send_numbers_and_sleep(&mut sender0, 0, 3);
        scars_test::test_fail()
    });

    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 2);
    // task3 not yet started before first yield-cycle between tasks in ready queue
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 3);
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 3);
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 3);
    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 0);
    assert_eq!(receiver.recv(), 0);
    assert_eq!(receiver.recv(), 0);
    scars_test::test_succeed();
}
