#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::CoreCeilingLock;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

// Lower priority thread
const LOW_PRIORITY: Priority = Priority::thread(3);

// Medium priority thread
const MEDIUM_PRIORITY: Priority = Priority::thread(4);

const CHECKER_PRIORITY: Priority = Priority::thread(1);

const CAPACITY: usize = 10;
const CEILING: Priority = MEDIUM_PRIORITY;

type LowThreadF = impl ThreadFn;
type MediumThreadF = impl ThreadFn;
type CheckerF = impl ThreadFn;

static LOW_STACK: Stack<STACK_SIZE> = Stack::new();
static LOW_THREAD: Thread<LOW_PRIORITY, LowThreadF> = Thread::new("low");

static MEDIUM_STACK: Stack<STACK_SIZE> = Stack::new();
static MEDIUM_THREAD: Thread<MEDIUM_PRIORITY, MediumThreadF> = Thread::new("medium");

static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER_THREAD: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

/// Ceiling lock prevents preemption by a thread at ceiling priority,
/// and when the lock is released, the thread will run.
#[scars::init]
#[define_opaque(LowThreadF, MediumThreadF, CheckerF)]
fn init() {
    let (sender0, receiver) = make_channel!(u32, CAPACITY, MEDIUM_PRIORITY);

    let medium_sender = sender0.clone();

    LOW_THREAD
        .init(LOW_STACK.init())
        .attach(move || {
            let lock: CoreCeilingLock<CEILING> = CoreCeilingLock::new();

            // Low priority thread raises its priority with a ceiling lock
            let pinned = core::pin::pin!(lock);
            let guard = pinned.as_ref().lock();

            let medium_sender_inner = medium_sender.clone();
            MEDIUM_THREAD
                .init(MEDIUM_STACK.init())
                .attach(move || {
                    medium_sender_inner.send(1);
                    loop {
                        scars::delay(Duration::from_secs(1));
                    }
                })
                .start();

            sender0.send(2);
            drop(guard);
            // Medium priority thread can run now, and then low priority continues
            sender0.send(0);
            loop {
                scars::delay(Duration::from_secs(1));
            }
        })
        .start();

    CHECKER_THREAD
        .init(CHECKER_STACK.init())
        .attach(move || {
            assert_eq!(receiver.recv(), 2);
            assert_eq!(receiver.recv(), 1);
            assert_eq!(receiver.recv(), 0);
            scars_test::test_succeed()
        })
        .start();
}
