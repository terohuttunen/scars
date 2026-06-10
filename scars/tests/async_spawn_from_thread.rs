//! Spawn a task onto a running executor from a thread. The thread captures
//! the executor's `ExecutorHandle` (which is `Send`) and spawns after the
//! scheduler has started, so the push goes to a live executor through the
//! atomic pending-ready queue, and the spawn alone must wake the executor —
//! no `send_events` is issued anywhere in this test.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::task::{EventHandlerExecutor, task_pool::TaskPool};
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const SPAWNER_PRIORITY: Priority = Priority::thread(2);

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();

type TaskF = impl core::future::Future<Output = ()>;
static POOL: TaskPool<TaskF, 1> = TaskPool::new();

type SpawnerF = impl ThreadFn;
static SPAWNER_STACK: Stack<STACK_SIZE> = Stack::new();
static SPAWNER: Thread<SPAWNER_PRIORITY, SpawnerF> = Thread::new("spawner");

#[define_opaque(TaskF)]
fn spawned_task() -> TaskF {
    async {
        scars_test::test_succeed();
    }
}

#[scars::init]
#[define_opaque(SpawnerF)]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let handle = executor.handle();

    SPAWNER
        .init(SPAWNER_STACK.init())
        .attach(move || {
            let _ = handle.spawn(POOL.alloc().unwrap().attach(spawned_task));
            loop {
                scars::delay_until(Instant::now() + Duration::from_secs(1));
            }
        })
        .start();
}
