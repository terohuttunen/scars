#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]
use scars::prelude::*;
use scars::time::{Duration, Instant};
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "hal-std"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "hal-std")]
const STACK_SIZE: usize = 16384;

// Higher priority task
const TASK0_PRIORITY: u8 = 3;

// Lower than higher priority task
const TASK1_PRIORITY: u8 = 2;

// Same priority as high priority task
const TASK2_PRIORITY: u8 = TASK0_PRIORITY;

const CAPACITY: usize = 20;
const CEILING: AnyPriority = any_task_priority(TASK0_PRIORITY);

/// Test that the scheduler preempts lower priority task when
/// a higher priority task becomes runnable from sleep, but does
/// not preempt higher priority task when lower or same priority
/// task becomes runnable.
#[test_case]
pub fn low_priority_task_does_not_preempt_high_priority() {
    #[derive(Debug, PartialEq, Eq, Clone)]
    pub enum Event {
        IdleStart,
        IdlePreemptedByTask0,
        IdleEnd,
        Task0Start,
        Task0PreemptAttemptByTask1,
        Task0PreemptAttemptByTask2,
        Task0End,
        Task1Start,
        Task1End,
        Task2Start,
        Task2End,
    }

    let (sender, receiver) = make_channel!(Event, CAPACITY, CEILING);
    sender.send(Event::IdleStart);

    let task0 = make_task!("task0", TASK0_PRIORITY, STACK_SIZE);

    let start_time = Instant::now();
    let wakeup_time = start_time + Duration::from_millis(50);
    let end_time = start_time + Duration::from_millis(100);
    let sender0 = sender.clone();
    task0.start(move || {
        sender0.send(Event::Task0Start);
        let end_time = wakeup_time + Duration::from_millis(100);

        let task1 = make_task!("task1", TASK1_PRIORITY, STACK_SIZE);
        let task2 = make_task!("task2", TASK2_PRIORITY, STACK_SIZE);
        let sender1 = sender0.clone();
        task1.start(move || {
            let wakeup_time = wakeup_time + Duration::from_millis(25);
            sender1.send(Event::Task1Start);
            let end_time = wakeup_time + Duration::from_millis(50);
            // Go to sleep until it is time to wake up to preempt the lower priority task0
            scars::delay_until(wakeup_time);
            sender1.send(Event::Task0PreemptAttemptByTask1);
            // Do some work until end_time
            while Instant::now() < end_time {}

            sender1.send(Event::Task1End);
            scars::delay_until(wakeup_time + Duration::from_secs(1));

            scars_test::test_fail()
        });

        let sender2 = sender0.clone();
        task2.start(move || {
            let wakeup_time = wakeup_time + Duration::from_millis(30);
            sender2.send(Event::Task2Start);
            let end_time = wakeup_time + Duration::from_millis(50);
            // Go to sleep until it is time to wake up to preempt the lower priority task0
            scars::delay_until(wakeup_time);
            sender2.send(Event::Task0PreemptAttemptByTask2);
            // Do some work until end_time
            while Instant::now() < end_time {}

            sender2.send(Event::Task2End);
            scars::delay_until(wakeup_time + Duration::from_secs(10));

            scars_test::test_fail()
        });

        // Go to sleep until it is time to wake up to preempt the idle task
        scars::delay_until(wakeup_time);
        // Idle task preempted
        let preempt_latency = wakeup_time.elapsed();
        assert!(preempt_latency < Duration::from_millis(10));
        sender0.send(Event::IdlePreemptedByTask0);

        // Do some work until end_time
        scars::printkln!("task0 working");
        while Instant::now() < end_time {}

        sender0.send(Event::Task0End);

        scars::delay_until(wakeup_time + Duration::from_secs(1));

        scars_test::test_fail()
    });
    // Do work until end time. The pre-emption should happen in the middle of the
    // the work around 50ms from the beginning.
    while Instant::now() < end_time {}
    sender.send(Event::IdleEnd);

    assert_eq!(receiver.recv(), Event::IdleStart);
    assert_eq!(receiver.recv(), Event::Task0Start);
    assert_eq!(receiver.recv(), Event::Task2Start);
    assert_eq!(receiver.recv(), Event::Task1Start);
    assert_eq!(receiver.recv(), Event::IdlePreemptedByTask0);
    assert_eq!(receiver.recv(), Event::Task0End);
    assert_eq!(receiver.recv(), Event::Task0PreemptAttemptByTask2); // <- Did not occur before task0 work ended
    assert_eq!(receiver.recv(), Event::Task2End);
    assert_eq!(receiver.recv(), Event::Task0PreemptAttemptByTask1); // <- Did not occur before task0 work ended
    assert_eq!(receiver.recv(), Event::Task1End);
    assert_eq!(receiver.recv(), Event::IdleEnd);
    scars_test::test_succeed();
}
