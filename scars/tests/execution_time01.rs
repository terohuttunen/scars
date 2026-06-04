#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn, ThreadRef};
use scars::time::Duration;
use scars::time::Instant;
use scars::time::execution_time::interrupt_clock;
use scars_test;

const STACK_SIZE: usize = 16384;

const CONTROLLER_PRIORITY: Priority = Priority::thread(10);
const WORKER_PRIORITY: Priority = Priority::thread(5);
const WATCHDOG_PRIORITY: Priority = Priority::thread(2);
const CEILING: Priority = CONTROLLER_PRIORITY;

const DONE: u8 = 1;

/// Wall-clock the worker busy-spins consuming CPU.
const BUSY: Duration = Duration::from_millis(100);
/// Window the controller waits while the worker is blocked.
const BLOCKED_WINDOW: Duration = Duration::from_millis(50);

type ControllerF = impl ThreadFn;
type WorkerF = impl ThreadFn;
type WatchdogF = impl ThreadFn;

static CONTROLLER_STACK: Stack<STACK_SIZE> = Stack::new();
static CONTROLLER: Thread<CONTROLLER_PRIORITY, ControllerF> = Thread::new("controller");
static WORKER_STACK: Stack<STACK_SIZE> = Stack::new();
static WORKER: Thread<WORKER_PRIORITY, WorkerF> = Thread::new("worker");
static WATCHDOG_STACK: Stack<STACK_SIZE> = Stack::new();
static WATCHDOG: Thread<WATCHDOG_PRIORITY, WatchdogF> = Thread::new("watchdog");

/// Per-thread execution-time accounting.
///
/// A low-priority worker busy-spins for `BUSY`, consuming CPU, then blocks
/// forever. The high-priority controller, blocked in `recv` meanwhile,
/// verifies:
///
/// - the worker accrued roughly `BUSY` of CPU time (a busy thread is charged),
/// - the controller itself accrued far less (a blocked thread is not charged
///   for the worker's work),
/// - while the worker is blocked its CPU clock does not advance, even across a
///   `BLOCKED_WINDOW` delay,
/// - the aggregate interrupt clock is a small fraction of the worker's CPU
///   time (interrupt time is excluded from thread time).
#[scars::init]
#[define_opaque(ControllerF, WorkerF, WatchdogF)]
fn init() {
    let (tx, rx) = make_channel!(u8, 2, CEILING);

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
            let tx = tx.clone();
            let worker_ref = WORKER
                .init(WORKER_STACK.init())
                .attach(move || {
                    // Pure-CPU busy spin: no syscalls, no blocking.
                    let t0 = Instant::now();
                    while t0.elapsed() < BUSY {
                        core::hint::spin_loop();
                    }
                    tx.send(DONE);
                    loop {
                        scars::delay(Duration::from_secs(60));
                    }
                })
                .start();

            // Block until the worker has finished its busy spin. While we are
            // blocked here the worker is the only runnable thread and consumes
            // CPU; sending charges its spin time at the switch back to us.
            assert_eq!(rx.recv(), DONE);

            let worker_busy = worker_ref.execution_time();
            let controller_time = unsafe { ThreadRef::current() }.execution_time();

            // The busy thread was charged most of its spin (lenient lower
            // bound for host scheduling jitter).
            assert!(worker_busy >= Duration::from_millis(50));
            // The thread blocked in recv was not charged the worker's CPU time.
            assert!(controller_time < worker_busy);
            // Interrupt time is tracked separately and excluded from thread CPU
            // time: the since-boot interrupt clock is far below the worker's
            // 100 ms of charged CPU.
            assert!(interrupt_clock() < worker_busy);

            // The worker is now blocked in its 60 s delay. Its CPU clock must
            // not advance while it is off-CPU, even across a delay window.
            let before = worker_ref.execution_time();
            scars::delay(BLOCKED_WINDOW);
            let after = worker_ref.execution_time();
            assert!(after - before < Duration::from_millis(25));

            scars_test::test_succeed()
        })
        .start();
}
