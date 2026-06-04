#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn, ThreadRef};
use scars::time::Duration;
use scars::{EventOptions, Events, WaitEvents};
use scars_test;

const STACK_SIZE: usize = 16384;

const CONTROLLER_PRIORITY: Priority = Priority::thread(10);
const WORKER_PRIORITY: Priority = Priority::thread(5);
const WATCHDOG_PRIORITY: Priority = Priority::thread(2);

/// CPU time the worker must consume before the monitor budget fires.
const BUDGET: Duration = Duration::from_millis(50);
/// Non-CPU (blocked) time the worker spends before consuming CPU. A correct
/// implementation must not count this against the budget.
const IDLE_DELAY: Duration = Duration::from_millis(100);

const BUDGET_EVENT: Events = 1;

type ControllerF = impl ThreadFn;
type WorkerF = impl ThreadFn;
type WatchdogF = impl ThreadFn;

static CONTROLLER_STACK: Stack<STACK_SIZE> = Stack::new();
static CONTROLLER: Thread<CONTROLLER_PRIORITY, ControllerF> = Thread::new("controller");
static WORKER_STACK: Stack<STACK_SIZE> = Stack::new();
static WORKER: Thread<WORKER_PRIORITY, WorkerF> = Thread::new("worker");
static WATCHDOG_STACK: Stack<STACK_SIZE> = Stack::new();
static WATCHDOG: Thread<WATCHDOG_PRIORITY, WatchdogF> = Thread::new("watchdog");

/// Execution-time monitor budget firing.
///
/// The worker is built with a `BUDGET` monitor, starts a window, then blocks
/// for `IDLE_DELAY` (consuming no CPU) before busy-spinning. The budget fires
/// only after the worker has run `BUDGET` of CPU, not while it is blocked. The
/// controller, woken by the budget event, checks that the worker's CPU clock
/// reads roughly `BUDGET`: blocked time does not count toward the budget.
#[scars::init]
#[define_opaque(ControllerF, WorkerF, WatchdogF)]
fn init() {
    WATCHDOG
        .init(WATCHDOG_STACK.init())
        .attach(move || {
            scars::delay(Duration::from_millis(3000));
            scars_test::test_fail()
        })
        .start();

    CONTROLLER
        .init(CONTROLLER_STACK.init())
        .attach(move || {
            let controller_sender = unsafe { ThreadRef::current() }.sender();

            let worker_ref = WORKER
                .init(WORKER_STACK.init())
                .monitor(BUDGET, controller_sender, BUDGET_EVENT)
                .attach(move || {
                    let me = unsafe { ThreadRef::current() };
                    me.restart_execution_time_monitor();
                    // Blocked time: must not count against the budget.
                    scars::delay(IDLE_DELAY);
                    // CPU time: the budget fires partway through this spin.
                    loop {
                        core::hint::spin_loop();
                    }
                })
                .start();

            // Wait for the budget to fire.
            WaitEvents::with_options(BUDGET_EVENT, EventOptions::wait_any()).wait();

            // It fired because the worker consumed CPU, not because wall time
            // (the IDLE_DELAY) elapsed: its CPU clock is ~BUDGET, well above
            // the near-zero it would read had blocked time been counted.
            let consumed = worker_ref.execution_time();
            assert!(consumed >= Duration::from_millis(40));
            assert!(consumed < Duration::from_millis(150));

            scars_test::test_succeed()
        })
        .start();
}
