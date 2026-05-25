#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::{BarrierResult, CoreCeilingLock, Notify, Protected, TimedOut};
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
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

static DATA: Protected<u32, CoreCeilingLock<CEILING>> = Protected::new(0);
static NOTIFY: Notify<CoreCeilingLock<CEILING>> = Notify::new();

/// Exercises `Protected::with_barrier_until` on both paths:
/// 1. The first wait times out (no notifier in window) → `Err(TimedOut)`.
/// 2. The second wait is notified before the deadline → `Ok(())`.
#[scars::init]
#[define_opaque(WaiterF, NotifierF, CheckerF)]
fn init() {
    let (done_tx, done_rx) = make_channel!(u8, 2, WAITER_PRIORITY);

    WAITER
        .init(WAITER_STACK.init())
        .attach(move || {
            // 1) Time out: no signaler will fire within this short window.
            let deadline = Instant::now() + Duration::from_millis(10);
            let r1 = DATA.with_barrier_until(deadline, |key, d| {
                if *d > 0 {
                    *d -= 1;
                    BarrierResult::Done(())
                } else {
                    BarrierResult::Wait(NOTIFY.arm(key))
                }
            });
            assert_eq!(r1, Err(TimedOut));
            done_tx.send(0);

            // 2) Successful wait: the notifier sets DATA + signals well
            // before this much longer deadline.
            let deadline = Instant::now() + Duration::from_secs(5);
            let r2 = DATA.with_barrier_until(deadline, |key, d| {
                if *d > 0 {
                    *d -= 1;
                    BarrierResult::Done(())
                } else {
                    BarrierResult::Wait(NOTIFY.arm(key))
                }
            });
            assert_eq!(r2, Ok(()));
            done_tx.send(1);
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    NOTIFIER
        .init(NOTIFIER_STACK.init())
        .attach(move || {
            // Wait for the waiter's timeout result first, so we don't
            // accidentally signal during phase (1).
            // After the timeout fires, deposit + signal for phase (2).
            scars::delay(Duration::from_millis(50));
            DATA.with(|_, d| {
                *d = 1;
                NOTIFY.notify_one();
            });
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    CHECKER
        .init(CHECKER_STACK.init())
        .attach(move || {
            assert_eq!(done_rx.recv(), 0); // phase 1 timed out
            assert_eq!(done_rx.recv(), 1); // phase 2 succeeded
            scars_test::test_succeed()
        })
        .start();
}
