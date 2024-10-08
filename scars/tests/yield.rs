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

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const THREAD0_PRIORITY: Priority = Priority::thread(3);

// Higher priority FIFO threads
const THREAD1_PRIORITY: Priority = Priority::thread(5);

const CAPACITY: usize = 14;
const CEILING: Priority = THREAD0_PRIORITY.max(THREAD1_PRIORITY);

/// When thread yields it never switches to lower priority thread, and will
/// alternate with same priority threads in FIFO order.
#[test_case]
pub fn yield_fifo() {
    fn send_numbers_and_sleep(
        sender: &mut Sender<u32, CAPACITY, CEILING>,
        number: u32,
        count: usize,
    ) {
        for _i in 0..count {
            sender.send(number);
            scars::thread_yield();
        }

        scars::delay(Duration::from_millis(1000));
    }

    let thread0 = make_thread!("thread0", THREAD0_PRIORITY, STACK_SIZE);
    let (mut sender0, receiver) = make_channel!(u32, CAPACITY, CEILING);
    thread0.start(move || {
        let thread1 = make_thread!("thread1", THREAD1_PRIORITY, STACK_SIZE);
        let mut sender1 = sender0.clone();
        thread1.start(move || {
            let thread2 = make_thread!("thread2", THREAD1_PRIORITY, STACK_SIZE);
            let mut sender2 = sender1.clone();
            thread2.start(move || {
                let thread3 = make_thread!("thread3", THREAD1_PRIORITY, STACK_SIZE);
                let mut sender3 = sender2.clone();
                thread3.start(move || {
                    send_numbers_and_sleep(&mut sender3, 3, 3);
                    scars_test::test_fail();
                });

                send_numbers_and_sleep(&mut sender2, 2, 4);
                scars_test::test_fail();
            });

            send_numbers_and_sleep(&mut sender1, 1, 4);
            scars_test::test_fail();
        });
        // Higher priority threads will block execution of this thread after thread1 is started
        send_numbers_and_sleep(&mut sender0, 0, 3);
        scars_test::test_fail()
    });

    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 2);
    // thread3 not yet started before first yield-cycle between threads in ready queue
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
