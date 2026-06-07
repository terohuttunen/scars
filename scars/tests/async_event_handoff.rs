//! The boundary case: an external event (here delivered via `send_events`,
//! standing in for an interrupt) wakes task A through `WaitForEvents`; A then
//! hands a value to task B over an `AsyncChannel`. Shows interrupt->task via
//! events composing with task->task via an async primitive.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::events::Events;
use scars::prelude::*;
use scars::sync::AsyncChannel;
use scars::task::{EventHandlerExecutor, WaitForEvents, task_pool::TaskPool};

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const TRIGGER: Events = 1 << 0;
const VALUE: u32 = 0xABCD;

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();
static CHANNEL: AsyncChannel<u32, 4> = AsyncChannel::new();

type SourceF = impl core::future::Future<Output = ()>;
type SinkF = impl core::future::Future<Output = ()>;
static SOURCE_POOL: TaskPool<SourceF, 1> = TaskPool::new();
static SINK_POOL: TaskPool<SinkF, 1> = TaskPool::new();

#[define_opaque(SourceF)]
fn source(channel: &'static AsyncChannel<u32, 4>) -> SourceF {
    async move {
        // Wait for the external stimulus, then forward to the sink task.
        WaitForEvents::new(TRIGGER).await;
        channel.send(VALUE).await;
    }
}

#[define_opaque(SinkF)]
fn sink(channel: &'static AsyncChannel<u32, 4>) -> SinkF {
    async move {
        let item = channel.recv().await;
        assert_eq!(item, VALUE);
        scars_test::test_succeed();
    }
}

#[scars::init]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let _ = executor.spawn(SOURCE_POOL.alloc().unwrap().attach(|| source(&CHANNEL)));
    let _ = executor.spawn(SINK_POOL.alloc().unwrap().attach(|| sink(&CHANNEL)));
    // The external stimulus (an interrupt would call this from its handler).
    executor.send_events(TRIGGER);
}
