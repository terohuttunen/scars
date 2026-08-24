#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn, ThreadRef};
use scars::time::Duration;
use scars::{EventOptions, Events, WaitEvents};
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024 * 2;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// The waiter runs above the sender, so it is already blocked in the wait
// by the time the sender gets to run.
const WAITER_PRIORITY: Priority = Priority::thread(5);
const SENDER_PRIORITY: Priority = Priority::thread(3);

const EVENT_A: Events = 1u32 << 0;
const EVENT_B: Events = 1u32 << 1;

type WaiterF = impl ThreadFn;
type SenderF = impl ThreadFn;

static WAITER_STACK: Stack<STACK_SIZE> = Stack::new();
static WAITER: Thread<WAITER_PRIORITY, WaiterF> = Thread::new("waiter");

static SENDER_STACK: Stack<STACK_SIZE> = Stack::new();
static SENDER: Thread<SENDER_PRIORITY, SenderF> = Thread::new("sender");

/// A wait that blocks returns the events that released it, and takes them
/// out of the thread's pending set.
#[scars::init]
#[define_opaque(WaiterF, SenderF)]
fn init() {
    WAITER
        .init(WAITER_STACK.init())
        .attach(|| {
            let waiter_ref = unsafe { ThreadRef::current() };

            SENDER
                .init(SENDER_STACK.init())
                .attach(move || {
                    // Releases the `wait_any` below. The waiter preempts on
                    // this call and runs up to its next wait.
                    waiter_ref.send_events(EVENT_A);

                    // Neither of these alone satisfies the `wait_all`; the
                    // waiter is released only once both have arrived.
                    waiter_ref.send_events(EVENT_A);
                    waiter_ref.send_events(EVENT_B);

                    loop {
                        scars::delay(Duration::from_secs(60));
                    }
                })
                .start();

            // Blocks: nothing is pending yet.
            let mut any = WaitEvents::with_options(EVENT_A, EventOptions::wait_any());
            assert_eq!(any.wait(), EVENT_A);
            // The wait consumed the event instead of leaving it pending.
            assert_eq!(any.peek(), 0);

            // Blocks until both events have been sent, and reports both.
            let mut all = WaitEvents::with_events(EVENT_A | EVENT_B);
            assert_eq!(all.wait(), EVENT_A | EVENT_B);
            assert_eq!(all.peek(), 0);

            scars_test::test_succeed()
        })
        .start();
}
