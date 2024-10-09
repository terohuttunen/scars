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

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const THREAD0_PRIORITY: Priority = Priority::thread(1);
const THREAD1_PRIORITY: Priority = Priority::thread(2);
const THREAD2_PRIORITY: Priority = Priority::thread(3);

#[test_case]
pub fn make_thread() {
    let thread0 = make_thread!("thread0", THREAD0_PRIORITY, STACK_SIZE);
    assert_eq!(thread0.name(), "thread0");
    assert_eq!(thread0.base_priority(), THREAD0_PRIORITY);
    assert_eq!(thread0.stack_ref().alloc_size(), STACK_SIZE);

    let thread1 = make_thread!("thread1", THREAD1_PRIORITY, STACK_SIZE);
    assert_eq!(thread1.name(), "thread1");
    assert_eq!(thread1.base_priority(), THREAD1_PRIORITY);
    assert_eq!(thread1.stack_ref().alloc_size(), STACK_SIZE);

    thread0.start(|| {
        assert!(true);
        loop {
            scars::delay_until(Instant::now() + Duration::from_secs(1));
        }
    });

    let u = 1234;
    thread1.start(move || {
        assert_eq!(u, 1234);
        assert!(true);
        let v = u;

        let thread2 = make_thread!("thread2", THREAD2_PRIORITY, STACK_SIZE);
        assert_eq!(thread2.name(), "thread2");
        assert_eq!(thread2.base_priority(), THREAD2_PRIORITY);
        assert_eq!(thread2.stack_ref().alloc_size(), STACK_SIZE);
        thread2.start(move || {
            assert_eq!(v, 1234);
            assert!(true);
            scars_test::test_succeed()
        });
        assert!(false);
        loop {}
    });
    assert!(false);
}
