#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::thread_suspend;
use scars::{EventOptions, Events, WaitEvents};
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024 * 2;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const THREAD0_PRIORITY: Priority = Priority::thread(3);

const CAPACITY: usize = 14;
const CEILING: Priority = THREAD0_PRIORITY;

const UNBLOCK_EVENT: Events = 1u32;

type Thread0F = impl ThreadFn;

static THREAD0_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD0: Thread<THREAD0_PRIORITY, Thread0F> = Thread::new("thread0");

/// Block a thread waiting for event and let it timeout
#[test_case]
#[define_opaque(Thread0F)]
pub fn block_waiting_event() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    THREAD0
        .init(THREAD0_STACK.init())
        .attach(move || {
            let deadline = scars::time::Instant::now() + scars::time::Duration::from_millis(10);
            let mut context = WaitEvents::with_options(UNBLOCK_EVENT, EventOptions::wait_any());
            let wait_result = context.wait_until(deadline);
            assert!(wait_result.is_err());
            sender.send(0);
            thread_suspend();
            loop {}
        })
        .start();

    assert_eq!(receiver.recv(), 0);
}
