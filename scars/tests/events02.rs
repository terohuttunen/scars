#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn, ThreadRef};
use scars::time::Duration;
use scars::{EventOptions, Events, WaitEvents};
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

const UNBLOCK_EVENT1: Events = 1u32;
const UNBLOCK_EVENT2: Events = 2u32;

type Thread0F = impl ThreadFn;
type Thread1F = impl ThreadFn;
type Thread2F = impl ThreadFn;

static THREAD0_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD0: Thread<THREAD0_PRIORITY, Thread0F> = Thread::new("thread0");

static THREAD1_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD1: Thread<THREAD1_PRIORITY, Thread1F> = Thread::new("thread1");

static THREAD2_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD2: Thread<THREAD2_PRIORITY, Thread2F> = Thread::new("thread2");

/// Block a thread waiting for event and release it with an event.
/// Highest priority thread ready to run will be woken up first.
#[test_case]
#[define_opaque(Thread0F, Thread1F, Thread2F)]
pub fn block_waiting_event() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    THREAD0
        .init(THREAD0_STACK.init())
        .attach(move || {
            let thread0_ref = unsafe { ThreadRef::current() };

            let sender1 = sender.clone();
            THREAD1
                .init(THREAD1_STACK.init())
                .attach(move || {
                    let thread1_ref = unsafe { ThreadRef::current() };

                    let sender2 = sender1.clone();
                    THREAD2
                        .init(THREAD2_STACK.init())
                        .attach(move || {
                            thread0_ref.send_events(UNBLOCK_EVENT1);
                            thread1_ref.send_events(UNBLOCK_EVENT1);
                            sender2.send(2);
                            thread0_ref.send_events(UNBLOCK_EVENT2);
                            thread1_ref.send_events(UNBLOCK_EVENT2);
                            sender2.send(3);
                            scars::delay(Duration::from_millis(1000));
                            scars_test::test_fail()
                        })
                        .start();

                    WaitEvents::with_options(
                        UNBLOCK_EVENT1 | UNBLOCK_EVENT2,
                        EventOptions::wait_all(),
                    )
                    .wait();
                    sender1.send(1);
                    scars::delay(Duration::from_millis(1000));
                    scars_test::test_fail()
                })
                .start();

            WaitEvents::with_options(UNBLOCK_EVENT1 | UNBLOCK_EVENT2, EventOptions::wait_all())
                .wait();
            sender.send(0);
            scars::delay(Duration::from_millis(1000));
            scars_test::test_fail()
        })
        .start();

    assert_eq!(receiver.recv(), 2);
    assert_eq!(receiver.recv(), 1);
    assert_eq!(receiver.recv(), 3);
    assert_eq!(receiver.recv(), 0);
}
