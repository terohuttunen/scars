//! `timeout` both ways: a short deadline around long work yields `Err(Elapsed)`,
//! and a long deadline around short work yields `Ok`. Running both back-to-back
//! in one task also exercises the per-task timer slot self-correcting between
//! the dropped loser and the next registration (§5.3).
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::prelude::*;
use scars::task::{EventHandlerExecutor, Sleep, task_pool::TaskPool, timeout};
use scars::time::Duration;

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const WAKE: scars::events::Events = 1 << 0;

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();

type TaskF = impl core::future::Future<Output = ()>;
static POOL: TaskPool<TaskF, 1> = TaskPool::new();

#[define_opaque(TaskF)]
fn task() -> TaskF {
    async {
        // Work outlasts the deadline -> timed out.
        let timed_out = timeout(
            Duration::from_millis(5),
            Sleep::sleep(Duration::from_millis(200)),
        )
        .await;
        assert!(timed_out.is_err());

        // Work finishes within the deadline -> Ok.
        let completed = timeout(
            Duration::from_millis(200),
            Sleep::sleep(Duration::from_millis(5)),
        )
        .await;
        assert!(completed.is_ok());

        scars_test::test_succeed();
    }
}

#[scars::init]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let _ = executor.spawn(POOL.alloc().unwrap().attach(task));
    executor.send_events(WAKE);
}
