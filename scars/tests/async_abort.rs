//! `JoinHandle::abort` cancels a running task: its future is dropped and it is
//! never polled again. A controller task lets a periodic worker run, aborts it
//! (while it is sleeping), and verifies the counter stops advancing.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use core::sync::atomic::{AtomicU32, Ordering};
use scars::prelude::*;
use scars::task::{EventHandlerExecutor, JoinHandle, Sleep, task_pool::TaskPool};
use scars::time::Duration;

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const WAKE: scars::events::Events = 1 << 0;
const TICK: Duration = Duration::from_millis(2);

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();
static COUNTER: AtomicU32 = AtomicU32::new(0);

type WorkerF = impl core::future::Future<Output = ()>;
type ControllerF = impl core::future::Future<Output = ()>;
static WORKER_POOL: TaskPool<WorkerF, 1> = TaskPool::new();
static CONTROLLER_POOL: TaskPool<ControllerF, 1> = TaskPool::new();

#[define_opaque(WorkerF)]
fn worker() -> WorkerF {
    async {
        loop {
            COUNTER.fetch_add(1, Ordering::SeqCst);
            Sleep::sleep(TICK).await;
        }
    }
}

#[define_opaque(ControllerF)]
fn controller(worker_handle: JoinHandle<()>) -> ControllerF {
    async move {
        // Let the worker run (and park on its sleep) for a while.
        Sleep::sleep(TICK * 5u32).await;
        worker_handle.abort();
        let stopped_at = COUNTER.load(Ordering::SeqCst);
        // Give it well over a tick: a cancelled task must not advance.
        Sleep::sleep(TICK * 5u32).await;
        assert_eq!(COUNTER.load(Ordering::SeqCst), stopped_at);
        scars_test::test_succeed();
    }
}

#[scars::init]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let worker_handle = executor.spawn(WORKER_POOL.alloc().unwrap().attach(worker));
    let _ = executor.spawn(
        CONTROLLER_POOL
            .alloc()
            .unwrap()
            .attach(move || controller(worker_handle)),
    );
    executor.send_events(WAKE);
}
