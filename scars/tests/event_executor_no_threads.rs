//! Threads-off smoke test for the event-handler async executor.
//!
//! Runs an async task on an [`EventHandlerExecutor`] (interrupt-context
//! executor) with NO user threads. The task sleeps via [`Sleep`], which
//! arms the kernel [`EventTimer`]; when the deadline elapses the timer
//! delivers the wakeup event to the executor's event handler, which
//! re-polls the task to completion. This exercises the whole minimal
//! kernel path — idle thread + service call + event handler + timer
//! queue — without any threading, and also passes with `threads` on.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::events::Events;
use scars::prelude::*;
use scars::task::{EventHandlerExecutor, Sleep, WaitForEvents, task_pool::TaskPool};
use scars::time::Duration;
use scars_test;

const EXECUTOR_PRIORITY: Priority = Priority::interrupt(1);
const TRIGGER: Events = 1 << 0;

type TaskF = impl core::future::Future<Output = ()>;

static EXECUTOR: EventHandlerExecutor<EXECUTOR_PRIORITY> = EventHandlerExecutor::new();
static POOL: TaskPool<TaskF, 4> = TaskPool::new();

#[scars::init]
#[define_opaque(TaskF)]
fn init() {
    let executor = EXECUTOR.init().publish().build();
    let task = POOL.alloc().unwrap().attach(|| async move {
        // Woken by the kick below (proves the executor polls tasks).
        WaitForEvents::new(TRIGGER).await;
        // Then sleep on the kernel timer: the EventTimer delivers the
        // wakeup event back to this executor's handler when the deadline
        // elapses — the timer->event-handler path, all without threads.
        Sleep::sleep(Duration::from_millis(10)).await;
        scars_test::test_succeed();
    });
    let _ = executor.spawn(task);
    // Kick the executor so it runs the task to its first await point.
    executor.send_events(TRIGGER);
}
