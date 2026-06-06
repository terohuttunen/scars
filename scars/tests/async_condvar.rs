//! An async consumer `wait_while`s on a condition behind an `AsyncMutex`; a
//! producer increments the value and `notify_one`s after each step. The
//! consumer must wake and observe the target value.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::prelude::*;
use scars::sync::{AsyncCondvar, AsyncMutex};
use scars::task::{EventHandlerExecutor, task_pool::TaskPool, yield_now};

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const TARGET: u32 = 5;
const WAKE: scars::events::Events = 1 << 0;

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();
static VALUE: AsyncMutex<u32> = AsyncMutex::new(0);
static READY: AsyncCondvar = AsyncCondvar::new();

type ConsumerF = impl core::future::Future<Output = ()>;
type ProducerF = impl core::future::Future<Output = ()>;
static CONSUMER_POOL: TaskPool<ConsumerF, 1> = TaskPool::new();
static PRODUCER_POOL: TaskPool<ProducerF, 1> = TaskPool::new();

#[define_opaque(ConsumerF)]
fn consumer(value: &'static AsyncMutex<u32>, ready: &'static AsyncCondvar) -> ConsumerF {
    async move {
        let guard = value.lock().await;
        let guard = ready.wait_while(guard, |v| *v < TARGET).await;
        assert_eq!(*guard, TARGET);
        scars_test::test_succeed();
    }
}

#[define_opaque(ProducerF)]
fn producer(value: &'static AsyncMutex<u32>, ready: &'static AsyncCondvar) -> ProducerF {
    async move {
        for _ in 0..TARGET {
            {
                let mut guard = value.lock().await;
                *guard += 1;
            }
            ready.notify_one();
            yield_now().await;
        }
    }
}

#[scars::init]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let _ = executor.spawn(
        CONSUMER_POOL
            .alloc()
            .unwrap()
            .attach(|| consumer(&VALUE, &READY)),
    );
    let _ = executor.spawn(
        PRODUCER_POOL
            .alloc()
            .unwrap()
            .attach(|| producer(&VALUE, &READY)),
    );
    executor.send_events(WAKE);
}
