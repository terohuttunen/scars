#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const CONTROLLER_PRIORITY: Priority = Priority::thread(10);
const WORKER_PRIORITY: Priority = Priority::thread(5);
const WATCHDOG_PRIORITY: Priority = Priority::thread(2);
const CEILING: Priority = CONTROLLER_PRIORITY;

const WORKER_RAN: u8 = 1;

type ControllerF = impl ThreadFn;
type WorkerF = impl ThreadFn;
type WatchdogF = impl ThreadFn;

static CONTROLLER_STACK: Stack<STACK_SIZE> = Stack::new();
static CONTROLLER: Thread<CONTROLLER_PRIORITY, ControllerF> = Thread::new("controller");
static WORKER_STACK: Stack<STACK_SIZE> = Stack::new();
static WORKER: Thread<WORKER_PRIORITY, WorkerF> = Thread::new("worker");
static WATCHDOG_STACK: Stack<STACK_SIZE> = Stack::new();
static WATCHDOG: Thread<WATCHDOG_PRIORITY, WatchdogF> = Thread::new("watchdog");

/// `ThreadRef::suspend` and `ThreadRef::resume`.
///
/// The controller (high priority) starts the worker, which is held off below
/// it, and suspends it before it can run. While the controller then delays, a
/// correctly suspended worker produces nothing — so the channel stays empty.
/// After resume the worker runs and reports.
#[scars::init]
#[define_opaque(ControllerF, WorkerF, WatchdogF)]
fn init() {
    let (tx, rx) = make_channel!(u8, 2, CEILING);

    WATCHDOG
        .init(WATCHDOG_STACK.init())
        .attach(move || {
            scars::delay(Duration::from_millis(2000));
            scars_test::test_fail()
        })
        .start();

    CONTROLLER
        .init(CONTROLLER_STACK.init())
        .attach(move || {
            let tx = tx.clone();
            let worker_ref = WORKER
                .init(WORKER_STACK.init())
                .attach(move || {
                    tx.send(WORKER_RAN);
                    loop {
                        scars::delay(Duration::from_secs(60));
                    }
                })
                .start();

            // Worker is ready but held off (lower priority). Suspend it before
            // it runs.
            worker_ref.suspend();

            // A suspended worker stays out of scheduling: nothing is produced
            // while the controller delays.
            scars::delay(Duration::from_millis(50));
            assert!(rx.try_recv().is_err());

            // Resume; the worker now runs and reports.
            worker_ref.resume();
            assert_eq!(rx.recv(), WORKER_RAN);
            scars_test::test_succeed()
        })
        .start();
}
