#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(type_alias_impl_trait)]

use core::pin::Pin;
use core::sync::atomic::{AtomicU32, Ordering};
use scars::Stack;
use scars::prelude::*;
use scars::sync::InheritanceLock;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
use scars_test;

scars_test::integration_test!();

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const LOW_PRIORITY: Priority = Priority::thread(3);

// Higher priority thread
const HIGH_PRIORITY: Priority = Priority::thread(5);

// Medium priority thread
const MEDIUM_PRIORITY: Priority = Priority::thread(4);

static LOCK1: InheritanceLock = InheritanceLock::new();
static STATE: AtomicU32 = AtomicU32::new(0);

type LowThreadF = impl ThreadFn;
type MediumThreadF = impl ThreadFn;
type HighThreadF = impl ThreadFn;

static LOW_STACK: Stack<STACK_SIZE> = Stack::new();
static LOW_THREAD: Thread<LOW_PRIORITY, LowThreadF> = Thread::new("low");

static MEDIUM_STACK: Stack<STACK_SIZE> = Stack::new();
static MEDIUM_THREAD: Thread<MEDIUM_PRIORITY, MediumThreadF> = Thread::new("medium");

static HIGH_STACK: Stack<STACK_SIZE> = Stack::new();
static HIGH_THREAD: Thread<HIGH_PRIORITY, HighThreadF> = Thread::new("high");

/// Check that a high priority thread trying to acquire an inheritance lock
/// will increase the priority of the low priority thread that is holding the lock.
/// Also tests that medium priority thread cannot preempt a low priority thread that
/// has inherited the high priority.
#[test_case]
#[define_opaque(LowThreadF, MediumThreadF, HighThreadF)]
pub fn inheritance_lock_priority_increase() {
    LOW_THREAD
        .init(LOW_STACK.init())
        .attach(move || {
            let pinned_lock1 = Pin::static_ref(&LOCK1);

            let g = pinned_lock1.lock();

            HIGH_THREAD
                .init(HIGH_STACK.init())
                .attach(move || {
                    let lock1 = Pin::static_ref(&LOCK1);

                    let _g = lock1.lock();

                    STATE.store(1, Ordering::SeqCst);

                    loop {
                        scars::delay(Duration::from_secs(1));
                    }
                })
                .start();

            MEDIUM_THREAD
                .init(MEDIUM_STACK.init())
                .attach(move || {
                    // After high priority thread acquires the lock, writes 1 to STATE, and goes to sleep,
                    // medium priority thread can run and write 2 to STATE.
                    assert_eq!(STATE.load(Ordering::SeqCst), 1);
                    STATE.store(2, Ordering::SeqCst);

                    loop {
                        scars::delay(Duration::from_secs(1));
                    }
                })
                .start();

            // High priority thread cannot write 1 to STATE because low priority thread
            // is holding the lock. Medium priority thread cannot run because low priority
            // thread has inherited the high priority.
            assert_eq!(STATE.load(Ordering::SeqCst), 0);

            // Low priority thread releases the lock, allowing high priority thread to run.
            // Low priority thread disinherits the high priority.
            drop(g);

            // High priority thread can write 1 to STATE because the lock has been released.
            // Then medium priority thread can run, and write 2 to STATE.
            // Medium priority thread was able to run, so low priority thread was preempted
            // because its priority was dropped back to its base priority.
            assert_eq!(STATE.load(Ordering::SeqCst), 2);
            STATE.store(3, Ordering::SeqCst);

            loop {
                scars::delay(Duration::from_secs(1));
            }
        })
        .start();

    assert_eq!(STATE.load(Ordering::SeqCst), 3);
}
