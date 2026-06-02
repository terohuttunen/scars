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

const WORKER_PRIORITY: Priority = Priority::thread(5);
const CONTROLLER_PRIORITY: Priority = Priority::thread(10);
const WATCHDOG_PRIORITY: Priority = Priority::thread(2);
const CHECKER_PRIORITY: Priority = Priority::thread(1);
const CEILING: Priority = CONTROLLER_PRIORITY;

type W1F = impl ThreadFn;
type W2F = impl ThreadFn;
type W3F = impl ThreadFn;
type ControllerF = impl ThreadFn;
type WatchdogF = impl ThreadFn;
type CheckerF = impl ThreadFn;

static W1_STACK: Stack<STACK_SIZE> = Stack::new();
static W1: Thread<WORKER_PRIORITY, W1F> = Thread::new("w1");
static W2_STACK: Stack<STACK_SIZE> = Stack::new();
static W2: Thread<WORKER_PRIORITY, W2F> = Thread::new("w2");
static W3_STACK: Stack<STACK_SIZE> = Stack::new();
static W3: Thread<WORKER_PRIORITY, W3F> = Thread::new("w3");
static CTRL_STACK: Stack<STACK_SIZE> = Stack::new();
static CTRL: Thread<CONTROLLER_PRIORITY, ControllerF> = Thread::new("controller");
static WATCHDOG_STACK: Stack<STACK_SIZE> = Stack::new();
static WATCHDOG: Thread<WATCHDOG_PRIORITY, WatchdogF> = Thread::new("watchdog");
static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

/// Gate on which the three equal-priority workers block before any of them
/// has run, so the controller can make them all ready at once.
static GATE: Semaphore<CoreCeilingLock<CEILING>> = Semaphore::new(0);

/// Equal-priority threads released together must run in first-blocked-first-run
/// (FIFO) order. W1, W2, W3 block on `GATE` in that order; the controller
/// (higher priority) releases the gate three times, making all three ready
/// while it still runs, then steps down. The workers then run in their ready
/// order and append their id to the channel; the checker expects `1, 2, 3`.
#[scars::init]
#[define_opaque(W1F, W2F, W3F, ControllerF, WatchdogF, CheckerF)]
fn init() {
    let (order_tx, order_rx) = make_channel!(u8, 3, CEILING);

    let tx1 = order_tx.clone();
    W1.init(W1_STACK.init())
        .attach(move || {
            GATE.acquire();
            tx1.send(1);
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    let tx2 = order_tx.clone();
    W2.init(W2_STACK.init())
        .attach(move || {
            GATE.acquire();
            tx2.send(2);
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    let tx3 = order_tx.clone();
    W3.init(W3_STACK.init())
        .attach(move || {
            GATE.acquire();
            tx3.send(3);
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    WATCHDOG
        .init(WATCHDOG_STACK.init())
        .attach(move || {
            scars::delay(Duration::from_millis(2000));
            scars_test::test_fail()
        })
        .start();

    CHECKER
        .init(CHECKER_STACK.init())
        .attach(move || {
            assert_eq!(order_rx.recv(), 1);
            assert_eq!(order_rx.recv(), 2);
            assert_eq!(order_rx.recv(), 3);
            scars_test::test_succeed()
        })
        .start();

    // Started last, at the highest priority: release all three workers while
    // still running, then block so they run in ready order.
    CTRL.init(CTRL_STACK.init())
        .attach(move || {
            GATE.release();
            GATE.release();
            GATE.release();
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();
}
