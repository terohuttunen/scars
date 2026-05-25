#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::{CoreCeilingLock, Semaphore};
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const WAITER_PRIORITY: Priority = Priority::thread(5);
const SIGNALER_PRIORITY: Priority = Priority::thread(3);
const CHECKER_PRIORITY: Priority = Priority::thread(1);

const CEILING: Priority = WAITER_PRIORITY;

type WaiterF = impl ThreadFn;
type SignalerF = impl ThreadFn;
type CheckerF = impl ThreadFn;

static WAITER_STACK: Stack<STACK_SIZE> = Stack::new();
static WAITER: Thread<WAITER_PRIORITY, WaiterF> = Thread::new("waiter");
static SIGNALER_STACK: Stack<STACK_SIZE> = Stack::new();
static SIGNALER: Thread<SIGNALER_PRIORITY, SignalerF> = Thread::new("signaler");
static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

static SEM: Semaphore<CoreCeilingLock<CEILING>> = Semaphore::new(0);

/// Counting semaphore round-trip: WAITER blocks on `acquire` (initial
/// count = 0); SIGNALER releases; WAITER unblocks and forwards a done
/// signal to CHECKER over a channel.
#[scars::init]
#[define_opaque(WaiterF, SignalerF, CheckerF)]
fn init() {
    let (done_tx, done_rx) = make_channel!(u32, 1, WAITER_PRIORITY);

    WAITER
        .init(WAITER_STACK.init())
        .attach(move || {
            // try_acquire must fail before any release.
            assert!(!SEM.try_acquire());
            // Blocks until SIGNALER releases.
            SEM.acquire();
            done_tx.send(1);
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    SIGNALER
        .init(SIGNALER_STACK.init())
        .attach(move || {
            SEM.release();
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    CHECKER
        .init(CHECKER_STACK.init())
        .attach(move || {
            let v = done_rx.recv();
            assert_eq!(v, 1);
            // After acquire consumed the release, count is back to 0.
            assert_eq!(SEM.available(), 0);
            scars_test::test_succeed()
        })
        .start();
}
