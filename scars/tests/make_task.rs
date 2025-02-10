#![no_std]
#![no_main]
#![feature(sync_unsafe_cell)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(impl_trait_in_assoc_type)]
use scars::prelude::*;
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

#[scars::thread(name = "thread0", priority = THREAD0_PRIORITY, stack_size = STACK_SIZE)]
fn thread0() -> ! {
    assert!(true);
    loop {
        scars::delay_until(Instant::now() + Duration::from_secs(1));
    }
}

#[scars::thread(name = "thread1", priority = THREAD1_PRIORITY, stack_size = STACK_SIZE)]
fn thread1() -> ! {
    let u = 1234;
    assert_eq!(u, 1234);
    assert!(true);
    let v = u;

    thread2(v).start();

    assert!(false);
    loop {}
}

#[scars::thread(name = "thread2", priority = THREAD2_PRIORITY, stack_size = STACK_SIZE)]
fn thread2(v: u32) -> ! {
    assert_eq!(v, 1234);
    assert!(true);
    scars_test::test_succeed()
}

#[test_case]
pub fn make_thread() {
    let thread0 = thread0();
    assert_eq!(thread0.name(), "thread0");
    assert_eq!(thread0.base_priority(), THREAD0_PRIORITY);
    assert_eq!(thread0.stack_ref().alloc_size(), STACK_SIZE);

    let thread1 = thread1();
    assert_eq!(thread1.name(), "thread1");
    assert_eq!(thread1.base_priority(), THREAD1_PRIORITY);
    assert_eq!(thread1.stack_ref().alloc_size(), STACK_SIZE);

    thread0.start();
    thread1.start();
    assert!(false);
}
