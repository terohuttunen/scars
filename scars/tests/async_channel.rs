//! Bounded async channel with capacity 1: a producer sends 0..N, a consumer
//! receives them. The small capacity forces both the full-send and empty-recv
//! blocking paths. Items must arrive in order.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::prelude::*;
use scars::sync::AsyncChannel;
use scars::task::{EventHandlerExecutor, task_pool::TaskPool};

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const N: u32 = 8;
const WAKE: scars::events::Events = 1 << 0;

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();
static CHANNEL: AsyncChannel<u32, 1> = AsyncChannel::new();

type ProducerF = impl core::future::Future<Output = ()>;
type ConsumerF = impl core::future::Future<Output = ()>;
static PRODUCER_POOL: TaskPool<ProducerF, 1> = TaskPool::new();
static CONSUMER_POOL: TaskPool<ConsumerF, 1> = TaskPool::new();

#[define_opaque(ProducerF)]
fn producer(channel: &'static AsyncChannel<u32, 1>) -> ProducerF {
    async move {
        for i in 0..N {
            channel.send(i).await;
        }
    }
}

#[define_opaque(ConsumerF)]
fn consumer(channel: &'static AsyncChannel<u32, 1>) -> ConsumerF {
    async move {
        for expected in 0..N {
            let item = channel.recv().await;
            assert_eq!(item, expected);
        }
        scars_test::test_succeed();
    }
}

#[scars::init]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let _ = executor.spawn(PRODUCER_POOL.alloc().unwrap().attach(|| producer(&CHANNEL)));
    let _ = executor.spawn(CONSUMER_POOL.alloc().unwrap().attach(|| consumer(&CHANNEL)));
    executor.send_events(WAKE);
}
