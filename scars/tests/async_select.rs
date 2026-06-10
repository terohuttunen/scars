//! `select!` a fast vs a slow timer. The fast branch must win, and the slow
//! `Sleep` (dropped as the loser) must not resurrect the completed task — the
//! test simply completing covers the cleanup path (§5.5).
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::prelude::*;
use scars::task::{Either, EventHandlerExecutor, Sleep, task_pool::TaskPool};
use scars::time::Duration;

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const WAKE: scars::events::Events = 1 << 0;

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();

type TaskF = impl core::future::Future<Output = ()>;
static POOL: TaskPool<TaskF, 1> = TaskPool::new();

#[define_opaque(TaskF)]
fn task() -> TaskF {
    async {
        let winner = scars::select!(
            Sleep::sleep(Duration::from_millis(5)),
            Sleep::sleep(Duration::from_millis(200)),
        )
        .await;
        match winner {
            Either::First(()) => scars_test::test_succeed(),
            Either::Second(()) => panic!("slow branch won the select"),
        }
    }
}

#[scars::init]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let _ = executor.spawn(POOL.alloc().unwrap().attach(task));
    executor.send_events(WAKE);
}
