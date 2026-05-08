#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::channel::CeilingSender;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const THREAD0_PRIORITY: Priority = Priority::thread(3);

// Higher priority FIFO threads
const THREAD1_PRIORITY: Priority = Priority::thread(5);

// Checker thread runs below the producers so it only drains the channel
// once they have made progress.
const CHECKER_PRIORITY: Priority = Priority::thread(1);

const CAPACITY: usize = 14;
const CEILING: Priority = THREAD0_PRIORITY.max(THREAD1_PRIORITY);

type Thread0F = impl ThreadFn;
type Thread1F = impl ThreadFn;
type Thread2F = impl ThreadFn;
type Thread3F = impl ThreadFn;
type CheckerF = impl ThreadFn;

static THREAD0_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD0: Thread<THREAD0_PRIORITY, Thread0F> = Thread::new("thread0");

static THREAD1_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD1: Thread<THREAD1_PRIORITY, Thread1F> = Thread::new("thread1");

static THREAD2_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD2: Thread<THREAD1_PRIORITY, Thread2F> = Thread::new("thread2");

static THREAD3_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD3: Thread<THREAD1_PRIORITY, Thread3F> = Thread::new("thread3");

static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER_THREAD: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

fn send_numbers_and_sleep(
    sender: &mut CeilingSender<u32, CAPACITY, CEILING>,
    number: u32,
    count: usize,
) {
    for _i in 0..count {
        sender.send(number);
        scars::thread_yield();
    }

    scars::delay(Duration::from_millis(1000));
}

/// When thread yields it never switches to lower priority thread, and will
/// alternate with same priority threads in FIFO order.
#[scars::init]
#[define_opaque(Thread0F, Thread1F, Thread2F, Thread3F, CheckerF)]
fn init() {
    let (sender0, receiver) = make_channel!(u32, CAPACITY, CEILING);

    CHECKER_THREAD
        .init(CHECKER_STACK.init())
        .attach(move || {
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
            scars_test::test_succeed()
        })
        .start();

    let sender_for_thread0 = sender0.clone();
    THREAD0
        .init(THREAD0_STACK.init())
        .attach(move || {
            let mut sender0 = sender_for_thread0.clone();

            let sender_for_thread1 = sender0.clone();
            THREAD1
                .init(THREAD1_STACK.init())
                .attach(move || {
                    let mut sender1 = sender_for_thread1.clone();

                    let sender_for_thread2 = sender1.clone();
                    THREAD2
                        .init(THREAD2_STACK.init())
                        .attach(move || {
                            let mut sender2 = sender_for_thread2.clone();

                            let sender_for_thread3 = sender2.clone();
                            THREAD3
                                .init(THREAD3_STACK.init())
                                .attach(move || {
                                    let mut sender3 = sender_for_thread3.clone();
                                    send_numbers_and_sleep(&mut sender3, 3, 3);
                                    scars_test::test_fail()
                                })
                                .start();

                            send_numbers_and_sleep(&mut sender2, 2, 4);
                            scars_test::test_fail()
                        })
                        .start();

                    send_numbers_and_sleep(&mut sender1, 1, 4);
                    scars_test::test_fail()
                })
                .start();

            send_numbers_and_sleep(&mut sender0, 0, 3);
            scars_test::test_fail()
        })
        .start();
}
