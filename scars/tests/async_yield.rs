//! Two tasks each increment a shared counter K times, calling `yield_now`
//! between steps. `yield_now` must reschedule the task (it wakes the executor
//! itself), and the two tasks must interleave.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use core::sync::atomic::{AtomicU32, Ordering};
use scars::prelude::*;
use scars::task::{EventHandlerExecutor, task_pool::TaskPool, yield_now};

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const K: u32 = 5;
const WAKE: scars::events::Events = 1 << 0;

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();
static COUNT: AtomicU32 = AtomicU32::new(0);
static LAST: AtomicU32 = AtomicU32::new(u32::MAX);
static SWITCHES: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicU32 = AtomicU32::new(0);

type WorkerF = impl core::future::Future<Output = ()>;
static POOL: TaskPool<WorkerF, 2> = TaskPool::new();

#[define_opaque(WorkerF)]
fn worker(id: u32) -> WorkerF {
    async move {
        for _ in 0..K {
            COUNT.fetch_add(1, Ordering::SeqCst);
            if LAST.swap(id, Ordering::SeqCst) != id {
                SWITCHES.fetch_add(1, Ordering::SeqCst);
            }
            yield_now().await;
        }
        if DONE.fetch_add(1, Ordering::SeqCst) == 1 {
            assert_eq!(COUNT.load(Ordering::SeqCst), 2 * K);
            // Both tasks ran turn-by-turn, so control switched repeatedly.
            assert!(SWITCHES.load(Ordering::SeqCst) >= 2);
            scars_test::test_succeed();
        }
    }
}

#[scars::init]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let _ = executor.spawn(POOL.alloc().unwrap().attach(|| worker(0)));
    let _ = executor.spawn(POOL.alloc().unwrap().attach(|| worker(1)));
    executor.send_events(WAKE);
}
