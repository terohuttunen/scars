#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::TimedOut;
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const WAITER_PRIORITY: Priority = Priority::thread(5);
const PRODUCER_PRIORITY: Priority = Priority::thread(3);
const CHECKER_PRIORITY: Priority = Priority::thread(1);

const CEILING: Priority = WAITER_PRIORITY;
const CAPACITY: usize = 1;

type WaiterF = impl ThreadFn;
type ProducerF = impl ThreadFn;
type CheckerF = impl ThreadFn;

static WAITER_STACK: Stack<STACK_SIZE> = Stack::new();
static WAITER: Thread<WAITER_PRIORITY, WaiterF> = Thread::new("waiter");
static PRODUCER_STACK: Stack<STACK_SIZE> = Stack::new();
static PRODUCER: Thread<PRODUCER_PRIORITY, ProducerF> = Thread::new("producer");
static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

/// Exercises `LockedChannel::recv_until` and `send_until` on both
/// paths: timeout-expires-first and operation-completes-before-deadline.
#[scars::init]
#[define_opaque(WaiterF, ProducerF, CheckerF)]
fn init() {
    // recv_until tests: PRODUCER → WAITER
    let (recv_tx, recv_rx) = make_channel!(u32, CAPACITY, CEILING);
    // send_until tests: WAITER → PRODUCER (capacity 1 lets us fill it)
    let (send_tx, send_rx) = make_channel!(u32, CAPACITY, CEILING);
    let (done_tx, done_rx) = make_channel!(u8, 4, WAITER_PRIORITY);

    WAITER
        .init(WAITER_STACK.init())
        .attach(move || {
            // 1) recv_until times out — no producer yet.
            let deadline = Instant::now() + Duration::from_millis(10);
            let r1 = recv_rx.recv_until(deadline);
            assert_eq!(r1, Err(TimedOut));
            done_tx.send(0);

            // 2) recv_until succeeds before the deadline.
            let deadline = Instant::now() + Duration::from_secs(5);
            let r2 = recv_rx.recv_until(deadline);
            assert_eq!(r2, Ok(42));
            done_tx.send(1);

            // 3) send_until times out — channel full + producer not
            // draining yet.
            send_tx.send(99); // fill the slot
            let deadline = Instant::now() + Duration::from_millis(10);
            let r3 = send_tx.send_until(100, deadline);
            assert_eq!(r3, Err(TimedOut));
            done_tx.send(2);

            // 4) send_until succeeds — producer drains the leftover
            // item, freeing the slot before the deadline.
            let deadline = Instant::now() + Duration::from_secs(5);
            let r4 = send_tx.send_until(101, deadline);
            assert_eq!(r4, Ok(()));
            done_tx.send(3);

            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    PRODUCER
        .init(PRODUCER_STACK.init())
        .attach(move || {
            // Let phase 1 time out, then deposit for phase 2.
            scars::delay(Duration::from_millis(50));
            recv_tx.send(42);

            // Let phase 3 time out, then drain send-side to unblock
            // phase 4. Two receives: one for the leftover 99, one for
            // phase 4's 101.
            scars::delay(Duration::from_millis(50));
            let _ = send_rx.recv();
            let _ = send_rx.recv();

            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    CHECKER
        .init(CHECKER_STACK.init())
        .attach(move || {
            assert_eq!(done_rx.recv(), 0);
            assert_eq!(done_rx.recv(), 1);
            assert_eq!(done_rx.recv(), 2);
            assert_eq!(done_rx.recv(), 3);
            scars_test::test_succeed()
        })
        .start();
}
