#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower than higher priority thread
const THREAD0_PRIORITY: Priority = Priority::thread(3);

// Higher priority thread
const THREAD1_PRIORITY: Priority = Priority::thread(4);

// Lowest worker priority — runs the busy loop that the higher-priority
// threads preempt, then drives the assertions.
const CHECKER_PRIORITY: Priority = Priority::thread(1);

const CAPACITY: usize = 10;
const CEILING: Priority = THREAD0_PRIORITY.max(THREAD1_PRIORITY);

type Thread0F = impl ThreadFn;
type Thread1F = impl ThreadFn;
type CheckerF = impl ThreadFn;

static THREAD0_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD0: Thread<THREAD0_PRIORITY, Thread0F> = Thread::new("thread0");

static THREAD1_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD1: Thread<THREAD1_PRIORITY, Thread1F> = Thread::new("thread1");

static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER_THREAD: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Event {
    IdleStart,
    IdlePreempt,
    IdleEnd,
    Thread0Start,
    Thread0Preempt,
    Thread0End,
    Thread1Start,
    Thread1End,
}

/// Test that the scheduler preempts lower priority thread when
/// a higher priority thread becomes runnable from sleep.
#[scars::init]
#[define_opaque(Thread0F, Thread1F, CheckerF)]
fn init() {
    CHECKER_THREAD
        .init(CHECKER_STACK.init())
        .attach(move || {
            let (sender, receiver) = make_channel!(Event, CAPACITY, CEILING);
            sender.send(Event::IdleStart);

            let start_time = Instant::now();
            let wakeup_time = start_time + Duration::from_millis(50);
            let end_time = start_time + Duration::from_millis(100);

            let sender0 = sender.clone();
            THREAD0
                .init(THREAD0_STACK.init())
                .attach(move || {
                    sender0.send(Event::Thread0Start);
                    let end_time = wakeup_time + Duration::from_millis(100);

                    let sender1 = sender0.clone();
                    let wakeup_time1 = wakeup_time + Duration::from_millis(25);
                    THREAD1
                        .init(THREAD1_STACK.init())
                        .attach(move || {
                            sender1.send(Event::Thread1Start);
                            let end_time = wakeup_time1 + Duration::from_millis(50);
                            // Go to sleep until it is time to wake up to preempt the lower priority thread0
                            scars::delay_until(wakeup_time1);
                            sender1.send(Event::Thread0Preempt);
                            // Do some work until end_time
                            while Instant::now() < end_time {}

                            sender1.send(Event::Thread1End);
                            scars::delay_until(wakeup_time1 + Duration::from_secs(1));

                            scars_test::test_fail()
                        })
                        .start();

                    // Go to sleep until it is time to wake up to preempt the idle thread
                    scars::delay_until(wakeup_time);
                    // Idle thread preempted
                    let preempt_latency = wakeup_time.elapsed();
                    assert!(preempt_latency < Duration::from_millis(10));
                    sender0.send(Event::IdlePreempt);

                    // Do some work until end_time
                    while Instant::now() < end_time {}

                    sender0.send(Event::Thread0End);

                    scars::delay_until(wakeup_time + Duration::from_secs(1));

                    scars_test::test_fail()
                })
                .start();

            // Do work until end time. The pre-emption should happen in the middle of the
            // the work around 50ms from the beginning.
            while Instant::now() < end_time {}
            sender.send(Event::IdleEnd);

            assert_eq!(receiver.recv(), Event::IdleStart);
            assert_eq!(receiver.recv(), Event::Thread0Start);
            assert_eq!(receiver.recv(), Event::Thread1Start);
            assert_eq!(receiver.recv(), Event::IdlePreempt);
            assert_eq!(receiver.recv(), Event::Thread0Preempt);
            assert_eq!(receiver.recv(), Event::Thread1End);
            assert_eq!(receiver.recv(), Event::Thread0End);
            assert_eq!(receiver.recv(), Event::IdleEnd);
            scars_test::test_succeed()
        })
        .start();
}
