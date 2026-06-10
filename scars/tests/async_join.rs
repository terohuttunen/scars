//! `join!` two timers in one task. This needs the executor's per-task timer
//! slot to merge multiple in-flight deadlines (§5.3): both `Sleep`s register,
//! the sooner fires, the later re-registers, and the join completes when both
//! are done.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::prelude::*;
use scars::task::{EventHandlerExecutor, Sleep, task_pool::TaskPool};
use scars::time::{Duration, Instant};

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const WAKE: scars::events::Events = 1 << 0;

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();

type TaskF = impl core::future::Future<Output = ()>;
static POOL: TaskPool<TaskF, 1> = TaskPool::new();

#[define_opaque(TaskF)]
fn task() -> TaskF {
    async {
        let start = Instant::now();
        let ((), ()) = scars::join!(
            Sleep::sleep(Duration::from_millis(5)),
            Sleep::sleep(Duration::from_millis(15)),
        )
        .await;
        // join resolves only once the later (15 ms) timer has also elapsed.
        assert!(Instant::now() >= start + Duration::from_millis(15));
        scars_test::test_succeed();
    }
}

#[scars::init]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let _ = executor.spawn(POOL.alloc().unwrap().attach(task));
    executor.send_events(WAKE);
}
