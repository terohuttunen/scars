#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::{CoreCeilingLock, Semaphore, TimedOut};
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const WAITER_PRIORITY: Priority = Priority::thread(5);
const RELEASER_PRIORITY: Priority = Priority::thread(3);
const CHECKER_PRIORITY: Priority = Priority::thread(1);

const CEILING: Priority = WAITER_PRIORITY;

type WaiterF = impl ThreadFn;
type ReleaserF = impl ThreadFn;
type CheckerF = impl ThreadFn;

static WAITER_STACK: Stack<STACK_SIZE> = Stack::new();
static WAITER: Thread<WAITER_PRIORITY, WaiterF> = Thread::new("waiter");
static RELEASER_STACK: Stack<STACK_SIZE> = Stack::new();
static RELEASER: Thread<RELEASER_PRIORITY, ReleaserF> = Thread::new("releaser");
static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

static SEM: Semaphore<CoreCeilingLock<CEILING>> = Semaphore::new(0);

/// Exercises `Semaphore::acquire_until` on both paths:
/// 1. Deadline elapses with no releaser → `Err(TimedOut)`.
/// 2. Releaser fires before the deadline → `Ok(())`.
#[scars::init]
#[define_opaque(WaiterF, ReleaserF, CheckerF)]
fn init() {
    let (done_tx, done_rx) = make_channel!(u8, 2, WAITER_PRIORITY);

    WAITER
        .init(WAITER_STACK.init())
        .attach(move || {
            // 1) Timeout: count is zero, nobody releases.
            let deadline = Instant::now() + Duration::from_millis(10);
            let r1 = SEM.acquire_until(deadline);
            assert_eq!(r1, Err(TimedOut));
            done_tx.send(0);

            // 2) Success: releaser bumps the count well within the
            // deadline.
            let deadline = Instant::now() + Duration::from_secs(5);
            let r2 = SEM.acquire_until(deadline);
            assert_eq!(r2, Ok(()));
            done_tx.send(1);

            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    RELEASER
        .init(RELEASER_STACK.init())
        .attach(move || {
            // Let phase 1 time out first.
            scars::delay(Duration::from_millis(50));
            SEM.release();
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
            scars_test::test_succeed()
        })
        .start();
}
