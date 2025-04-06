#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(impl_trait_in_assoc_type)]
use scars::prelude::*;
use scars::sync::channel::Sender;
use scars::time::{Duration, Instant};
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower than higher priority thread
const THREAD0_PRIORITY: Priority = Priority::thread(3);

// Higher priority thread
const THREAD1_PRIORITY: Priority = Priority::thread(4);

const CAPACITY: usize = 10;
const CEILING: Priority = THREAD0_PRIORITY.max(THREAD1_PRIORITY);

#[scars::thread(name = "thread0", priority = THREAD0_PRIORITY, stack_size = STACK_SIZE)]
fn thread0(sender: Sender<Event, CAPACITY, CEILING>, wakeup_time: Instant) -> ! {
    sender.send(Event::Thread0Start);
    let end_time = wakeup_time + Duration::from_millis(100);

    thread1(sender.clone(), wakeup_time + Duration::from_millis(25)).start();

    // Go to sleep until it is time to wake up to preempt the idle thread
    scars::delay_until(wakeup_time);
    // Idle thread preempted
    let preempt_latency = wakeup_time.elapsed();
    assert!(preempt_latency < Duration::from_millis(10));
    sender.send(Event::IdlePreempt);

    // Do some work until end_time
    while Instant::now() < end_time {}

    sender.send(Event::Thread0End);

    scars::delay_until(wakeup_time + Duration::from_secs(1));

    scars_test::test_fail()
}

#[scars::thread(name = "thread1", priority = THREAD1_PRIORITY, stack_size = STACK_SIZE)]
fn thread1(sender: Sender<Event, CAPACITY, CEILING>, wakeup_time: Instant) -> ! {
    sender.send(Event::Thread1Start);
    let end_time = wakeup_time + Duration::from_millis(50);
    // Go to sleep until it is time to wake up to preempt the lower priority thread0
    scars::delay_until(wakeup_time);
    sender.send(Event::Thread0Preempt);
    // Do some work until end_time
    while Instant::now() < end_time {}

    sender.send(Event::Thread1End);
    scars::delay_until(wakeup_time + Duration::from_secs(1));

    scars_test::test_fail()
}

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
#[test_case]
pub fn high_priority_thread_preempts_low_priority() {
    let (sender, receiver) = make_channel!(Event, CAPACITY, CEILING);
    sender.send(Event::IdleStart);

    let start_time = Instant::now();
    let wakeup_time = start_time + Duration::from_millis(50);
    let end_time = start_time + Duration::from_millis(100);

    thread0(sender.clone(), wakeup_time).start();

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
}
