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

const THREAD0_PRIORITY: Priority = Priority::thread(1);
const THREAD1_PRIORITY: Priority = Priority::thread(2);
const THREAD2_PRIORITY: Priority = Priority::thread(3);

type Thread0F = impl ThreadFn;
type Thread1F = impl ThreadFn;
type Thread2F = impl ThreadFn;

static THREAD0_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD0: Thread<THREAD0_PRIORITY, Thread0F> = Thread::new("thread0");

static THREAD1_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD1: Thread<THREAD1_PRIORITY, Thread1F> = Thread::new("thread1");

static THREAD2_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD2: Thread<THREAD2_PRIORITY, Thread2F> = Thread::new("thread2");

#[test_case]
#[define_opaque(Thread0F, Thread1F, Thread2F)]
pub fn make_thread() {
    let thread0 = THREAD0.init(THREAD0_STACK.init());
    assert_eq!(thread0.name(), "thread0");
    assert_eq!(thread0.base_priority(), THREAD0_PRIORITY);
    assert_eq!(thread0.stack_ref().alloc_size(), STACK_SIZE);

    let thread0_handle = thread0.attach(move || {
        assert!(true);
        loop {
            scars::delay_until(Instant::now() + Duration::from_secs(1));
        }
    });

    let thread1 = THREAD1.init(THREAD1_STACK.init());
    assert_eq!(thread1.name(), "thread1");
    assert_eq!(thread1.base_priority(), THREAD1_PRIORITY);
    assert_eq!(thread1.stack_ref().alloc_size(), STACK_SIZE);

    let thread1_handle = thread1.attach(move || {
        let v = 1234u32;
        assert_eq!(v, 1234);
        assert!(true);

        THREAD2
            .init(THREAD2_STACK.init())
            .attach(move || {
                assert_eq!(v, 1234);
                assert!(true);
                scars_test::test_succeed()
            })
            .start();

        assert!(false);
        loop {}
    });

    thread0_handle.start();
    thread1_handle.start();
    assert!(false);
}
