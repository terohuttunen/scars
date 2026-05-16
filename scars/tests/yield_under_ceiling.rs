#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::CeilingMutex;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const HOLDER_PRIORITY: Priority = Priority::thread(3);
const WAITER_PRIORITY: Priority = Priority::thread(5);
const CHECKER_PRIORITY: Priority = Priority::thread(1);
const MUTEX_CEILING: Priority = WAITER_PRIORITY;
const CHANNEL_CEILING: Priority = WAITER_PRIORITY;

type HolderF = impl ThreadFn;
type WaiterF = impl ThreadFn;
type CheckerF = impl ThreadFn;

static HOLDER_STACK: Stack<STACK_SIZE> = Stack::new();
static HOLDER: Thread<HOLDER_PRIORITY, HolderF> = Thread::new("holder");
static WAITER_STACK: Stack<STACK_SIZE> = Stack::new();
static WAITER: Thread<WAITER_PRIORITY, WaiterF> = Thread::new("waiter");
static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

static MUTEX: CeilingMutex<u32, MUTEX_CEILING> = CeilingMutex::new(0);

/// Regression test for the IPCP / `thread_yield` interaction:
///
/// HOLDER takes a ceiling-protected mutex, signals WAITER, then yields.
/// Under correct IPCP semantics, the yield must not hand the CPU to a
/// peer at the boosted (ceiling) priority — that peer could attempt the
/// same mutex and the kernel would panic at `ceiling_lock.rs`:
/// "Lock already owned. The scheduler should have prevented this."
///
/// Spawn order is load-bearing: WAITER must be blocked on
/// `signal_rx.recv` BEFORE HOLDER takes the mutex. Each `.start()` call
/// from init (idle context) immediately preempts to the higher-prio
/// newly-spawned thread, which runs until it blocks. So WAITER is
/// spawned first (runs, blocks on recv), CHECKER second (runs, blocks
/// on recv), HOLDER last (runs the lock/signal/yield/drop sequence with
/// WAITER already waiting for the signal).
#[scars::init]
#[define_opaque(HolderF, WaiterF, CheckerF)]
fn init() {
    let (signal_tx, signal_rx) = make_channel!(u8, 1, CHANNEL_CEILING);
    let (done_tx, done_rx) = make_channel!(u8, 1, CHANNEL_CEILING);

    WAITER
        .init(WAITER_STACK.init())
        .attach(move || {
            let _ = signal_rx.recv();
            let guard = MUTEX.lock();
            drop(guard);
            done_tx.send(0);
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    CHECKER
        .init(CHECKER_STACK.init())
        .attach(move || {
            let _ = done_rx.recv();
            scars_test::test_succeed()
        })
        .start();

    HOLDER
        .init(HOLDER_STACK.init())
        .attach(move || {
            let guard = MUTEX.lock();
            signal_tx.send(0);
            scars::thread_yield();
            drop(guard);
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();
}
