#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Higher priority thread
const THREAD0_PRIORITY: Priority = Priority::thread(3);

// Lower than higher priority thread
const THREAD1_PRIORITY: Priority = Priority::thread(2);

// Same priority as high priority thread
const THREAD2_PRIORITY: Priority = THREAD0_PRIORITY;

const CAPACITY: usize = 20;
const CEILING: Priority = THREAD0_PRIORITY.max(THREAD1_PRIORITY).max(THREAD2_PRIORITY);

type Thread0F = impl ThreadFn;
type Thread1F = impl ThreadFn;
type Thread2F = impl ThreadFn;

static THREAD0_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD0: Thread<THREAD0_PRIORITY, Thread0F> = Thread::new("thread0");

static THREAD1_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD1: Thread<THREAD1_PRIORITY, Thread1F> = Thread::new("thread1");

static THREAD2_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD2: Thread<THREAD2_PRIORITY, Thread2F> = Thread::new("thread2");

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Event {
    IdleStart,
    IdlePreemptedByThread0,
    IdleEnd,
    Thread0Start,
    Thread0PreemptAttemptByThread1,
    Thread0PreemptAttemptByThread2,
    Thread0End,
    Thread1Start,
    Thread1End,
    Thread2Start,
    Thread2End,
}

/// Test that the scheduler preempts lower priority thread when
/// a higher priority thread becomes runnable from sleep, but does
/// not preempt higher priority thread when lower or same priority
/// thread becomes runnable.
#[test_case]
#[define_opaque(Thread0F, Thread1F, Thread2F)]
pub fn low_priority_thread_does_not_preempt_high_priority() {
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
                    sender1.send(Event::Thread0PreemptAttemptByThread1);
                    // Do some work until end_time
                    while Instant::now() < end_time {}

                    sender1.send(Event::Thread1End);
                    scars::delay_until(wakeup_time1 + Duration::from_secs(1));

                    scars_test::test_fail()
                })
                .start();

            let sender2 = sender0.clone();
            let wakeup_time2 = wakeup_time + Duration::from_millis(30);
            THREAD2
                .init(THREAD2_STACK.init())
                .attach(move || {
                    sender2.send(Event::Thread2Start);
                    let end_time = wakeup_time2 + Duration::from_millis(50);
                    // Go to sleep until it is time to wake up to preempt the lower priority thread0
                    scars::delay_until(wakeup_time2);
                    sender2.send(Event::Thread0PreemptAttemptByThread2);
                    // Do some work until end_time
                    while Instant::now() < end_time {}

                    sender2.send(Event::Thread2End);
                    scars::delay_until(wakeup_time2 + Duration::from_secs(10));

                    scars_test::test_fail()
                })
                .start();

            // Go to sleep until it is time to wake up to preempt the idle thread
            scars::delay_until(wakeup_time);
            // Idle thread preempted
            let preempt_latency = wakeup_time.elapsed();
            assert!(preempt_latency < Duration::from_millis(10));
            sender0.send(Event::IdlePreemptedByThread0);

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
    assert_eq!(receiver.recv(), Event::Thread2Start);
    assert_eq!(receiver.recv(), Event::Thread1Start);
    assert_eq!(receiver.recv(), Event::IdlePreemptedByThread0);
    assert_eq!(receiver.recv(), Event::Thread0End);
    assert_eq!(receiver.recv(), Event::Thread0PreemptAttemptByThread2); // <- Did not occur before thread0 work ended
    assert_eq!(receiver.recv(), Event::Thread2End);
    assert_eq!(receiver.recv(), Event::Thread0PreemptAttemptByThread1); // <- Did not occur before thread0 work ended
    assert_eq!(receiver.recv(), Event::Thread1End);
    assert_eq!(receiver.recv(), Event::IdleEnd);
}
