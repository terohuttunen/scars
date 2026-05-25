#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::{BarrierResult, CoreCeilingLock, Notify, Protected};
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
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

/// `Notify<L>` paired with `Protected<u32, L>`: WAITER blocks until
/// DATA > 0; NOTIFIER sets DATA = 5 and signals; WAITER takes one and
/// forwards the taken value to CHECKER via a channel.
#[scars::init]
#[define_opaque(WaiterF, NotifierF, CheckerF)]
fn init() {
    let (done_tx, done_rx) = make_channel!(u32, 1, WAITER_PRIORITY);

    WAITER
        .init(WAITER_STACK.init())
        .attach(move || {
            let taken = DATA.with_barrier(|key, d| {
                if *d > 0 {
                    let v = *d;
                    *d -= 1;
                    BarrierResult::Done(v)
                } else {
                    BarrierResult::Wait(NOTIFY.arm(key))
                }
            });
            done_tx.send(taken);
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    NOTIFIER
        .init(NOTIFIER_STACK.init())
        .attach(move || {
            DATA.with(|_, d| {
                *d = 5;
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
            let v = done_rx.recv();
            assert_eq!(v, 5);
            scars_test::test_succeed()
        })
        .start();
}
