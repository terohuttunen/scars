#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::{CeilingCondvar, CeilingMutex, TimedOut};
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024 * 2;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const WAITER_PRIORITY: Priority = Priority::thread(5);
const NOTIFIER_PRIORITY: Priority = Priority::thread(3);
const CHECKER_PRIORITY: Priority = Priority::thread(1);

const CEILING: Priority = WAITER_PRIORITY;

type WaiterF = impl ThreadFn;
type NotifierF = impl ThreadFn;
type CheckerF = impl ThreadFn;

static WAITER_STACK: Stack<STACK_SIZE> = Stack::new();
static WAITER: Thread<WAITER_PRIORITY, WaiterF> = Thread::new("waiter");
static NOTIFIER_STACK: Stack<STACK_SIZE> = Stack::new();
static NOTIFIER: Thread<NOTIFIER_PRIORITY, NotifierF> = Thread::new("notifier");
static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

static LOCK: CeilingMutex<bool, CEILING> = CeilingMutex::new(false);
static CVAR: CeilingCondvar<CEILING> = CeilingCondvar::new();

/// Exercises `LockedCondvar::wait_while_until` on both paths:
/// 1. No notifier within the deadline → `WaitOutcome::TimedOut`.
/// 2. Notifier fires before the deadline → `WaitOutcome::Notified`.
#[scars::init]
#[define_opaque(WaiterF, NotifierF, CheckerF)]
fn init() {
    let (done_tx, done_rx) = make_channel!(u8, 2, WAITER_PRIORITY);

    WAITER
        .init(WAITER_STACK.init())
        .attach(move || {
            // 1) Timeout: condition stays false, no notifier.
            {
                let guard = LOCK.lock();
                let deadline = Instant::now() + Duration::from_millis(10);
                let r1 = CVAR.wait_while_until(guard, |b| !*b, deadline);
                assert!(matches!(r1, Err(TimedOut)));
            }
            done_tx.send(0);

            // 2) Success: notifier sets the flag and signals well
            // before this deadline.
            {
                let guard = LOCK.lock();
                let deadline = Instant::now() + Duration::from_secs(5);
                let g2 = CVAR.wait_while_until(guard, |b| !*b, deadline).unwrap();
                assert_eq!(*g2, true);
            }
            done_tx.send(1);

            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    NOTIFIER
        .init(NOTIFIER_STACK.init())
        .attach(move || {
            // Let phase 1 time out first.
            scars::delay(Duration::from_millis(50));
            *LOCK.lock() = true;
            CVAR.notify_one();
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
