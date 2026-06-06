//! Cooperative yield: hand the executor a chance to poll other ready tasks
//! before resuming.

use crate::task::task_and_executor;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

/// Yield once. The first poll re-queues the task and wakes the executor, so
/// other ready tasks run before this one is polled again; the second poll
/// completes.
pub fn yield_now() -> YieldNow {
    YieldNow {
        yielded: false,
        _not_send: PhantomData,
    }
}

pub struct YieldNow {
    yielded: bool,
    _not_send: PhantomData<*const ()>,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.yielded {
            Poll::Ready(())
        } else {
            this.yielded = true;
            // Re-queue this task and wake the executor so the next poll cycle
            // resumes it.
            let (task, executor) = task_and_executor(cx);
            executor.resume_task(task);
            executor.notify();
            Poll::Pending
        }
    }
}
