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

const TASK0_PRIORITY: u8 = 1;
const TASK1_PRIORITY: u8 = 2;
const TASK2_PRIORITY: u8 = 3;

#[test_case]
pub fn make_task() {
    let task0 = make_task!("task0", TASK0_PRIORITY, STACK_SIZE);
    assert_eq!(task0.name(), "task0");
    assert_eq!(
        task0.base_priority(),
        Priority::task_priority(TASK0_PRIORITY)
    );
    assert_eq!(task0.stack_ref().size(), STACK_SIZE);

    let task1 = make_task!("task1", TASK1_PRIORITY, STACK_SIZE);
    assert_eq!(task1.name(), "task1");
    assert_eq!(
        task1.base_priority(),
        Priority::task_priority(TASK1_PRIORITY)
    );
    assert_eq!(task1.stack_ref().size(), STACK_SIZE);

    task0.start(|| {
        assert!(true);
        loop {
            scars::delay_until(Instant::now() + Duration::from_secs(1));
        }
    });

    let u = 1234;
    task1.start(move || {
        assert_eq!(u, 1234);
        assert!(true);
        let v = u;

        let task2 = make_task!("task2", TASK2_PRIORITY, STACK_SIZE);
        assert_eq!(task2.name(), "task2");
        assert_eq!(
            task2.base_priority(),
            Priority::task_priority(TASK2_PRIORITY)
        );
        assert_eq!(task2.stack_ref().size(), STACK_SIZE);
        task2.start(move || {
            assert_eq!(v, 1234);
            assert!(true);
            scars_test::test_succeed()
        });
        assert!(false);
        loop {}
    });
    assert!(false);
}
