//! Two async tasks on one executor contend for an `AsyncMutex`, holding the
//! guard across a `yield_now` to force interleaving. If mutual exclusion held,
//! the read-modify-write loop has no lost updates and the final count is 2*K.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use core::sync::atomic::{AtomicU32, Ordering};
use scars::prelude::*;
use scars::sync::AsyncMutex;
use scars::task::{EventHandlerExecutor, task_pool::TaskPool, yield_now};

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const K: u32 = 10;
const WAKE: scars::events::Events = 1 << 0;

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();
static COUNTER: AsyncMutex<u32> = AsyncMutex::new(0);
static DONE: AtomicU32 = AtomicU32::new(0);

type WorkerF = impl core::future::Future<Output = ()>;
static POOL: TaskPool<WorkerF, 2> = TaskPool::new();

#[define_opaque(WorkerF)]
fn worker(counter: &'static AsyncMutex<u32>) -> WorkerF {
    async move {
        for _ in 0..K {
            let mut guard = counter.lock().await;
            let value = *guard;
            // Hold the lock across a yield: the peer must not observe or
            // mutate the counter while we hold the guard.
            yield_now().await;
            *guard = value + 1;
        }
        if DONE.fetch_add(1, Ordering::SeqCst) == 1 {
            let guard = counter.lock().await;
            assert_eq!(*guard, 2 * K);
            scars_test::test_succeed();
        }
    }
}

#[scars::init]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let _ = executor.spawn(POOL.alloc().unwrap().attach(|| worker(&COUNTER)));
    let _ = executor.spawn(POOL.alloc().unwrap().attach(|| worker(&COUNTER)));
    executor.send_events(WAKE);
}
