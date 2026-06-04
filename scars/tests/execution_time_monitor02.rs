#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Events;
use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn, ThreadRef};
use scars::time::{Duration, Instant};
use scars_test;

const STACK_SIZE: usize = 16384;

const CONTROLLER_PRIORITY: Priority = Priority::thread(10);
const WORKER_PRIORITY: Priority = Priority::thread(5);
const WATCHDOG_PRIORITY: Priority = Priority::thread(2);
const CEILING: Priority = CONTROLLER_PRIORITY;

const DONE: u8 = 1;

/// Initialization-period CPU. It runs before the first restart, so it is in no
/// measurement window and must not appear in the recorded WCET.
const INIT: Duration = Duration::from_millis(100);
/// Per-cycle CPU.
const CYCLE: Duration = Duration::from_millis(30);
/// A budget high enough that cyclic work never trips it.
const BUDGET: Duration = Duration::from_millis(500);
const CYCLES: usize = 3;
const OVERRUN: Events = 1;

type ControllerF = impl ThreadFn;
type WorkerF = impl ThreadFn;
type WatchdogF = impl ThreadFn;

static CONTROLLER_STACK: Stack<STACK_SIZE> = Stack::new();
static CONTROLLER: Thread<CONTROLLER_PRIORITY, ControllerF> = Thread::new("controller");
static WORKER_STACK: Stack<STACK_SIZE> = Stack::new();
static WORKER: Thread<WORKER_PRIORITY, WorkerF> = Thread::new("worker");
static WATCHDOG_STACK: Stack<STACK_SIZE> = Stack::new();
static WATCHDOG: Thread<WATCHDOG_PRIORITY, WatchdogF> = Thread::new("watchdog");

fn spin(duration: Duration) {
    let t0 = Instant::now();
    while t0.elapsed() < duration {
        core::hint::spin_loop();
    }
}

/// Per-window worst-case CPU measurement, initialization exclusion, and reset.
///
/// The worker runs a long initialization phase, then starts its monitor and
/// runs several shorter cyclic iterations, restarting the window each cycle.
/// The recorded WCET reflects one cyclic window, not the longer init phase:
/// init runs before the first restart and so belongs to no window. The
/// controller then resets the WCET and confirms it clears.
#[scars::init]
#[define_opaque(ControllerF, WorkerF, WatchdogF)]
fn init() {
    let (tx, rx) = make_channel!(u8, 2, CEILING);

    WATCHDOG
        .init(WATCHDOG_STACK.init())
        .attach(move || {
            scars::delay(Duration::from_millis(4000));
            scars_test::test_fail()
        })
        .start();

    CONTROLLER
        .init(CONTROLLER_STACK.init())
        .attach(move || {
            let sender = unsafe { ThreadRef::current() }.sender();
            let tx = tx.clone();

            let worker_ref = WORKER
                .init(WORKER_STACK.init())
                .monitor(BUDGET, sender, OVERRUN)
                .attach(move || {
                    let me = unsafe { ThreadRef::current() };
                    // Initialization period: not yet monitored.
                    spin(INIT);
                    // Start the monitor and run cyclic work, restarting the
                    // window each cycle so the previous window is measured.
                    me.restart_execution_time_monitor();
                    for _ in 0..CYCLES {
                        spin(CYCLE);
                        me.restart_execution_time_monitor();
                    }
                    tx.send(DONE);
                    loop {
                        scars::delay(Duration::from_secs(60));
                    }
                })
                .start();

            assert_eq!(rx.recv(), DONE);

            // The worst window is one cyclic iteration (~CYCLE), well below the
            // ~INIT initialization phase; the high budget never fired.
            let wcet = worker_ref.wcet();
            assert!(wcet >= Duration::from_millis(20));
            assert!(wcet < Duration::from_millis(70));

            // Reset clears the recorded WCET.
            worker_ref.reset_wcet();
            assert_eq!(worker_ref.wcet(), Duration::ZERO);

            scars_test::test_succeed()
        })
        .start();
}
