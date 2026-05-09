use super::raw_task::RawTask;
use crate::time::{Duration, Instant};
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

pub struct Sleep {
    deadline: Instant,
    // To make sure that Timer is not Send or Sync
    _phantom: PhantomData<*const ()>,
}

impl Sleep {
    pub fn sleep(duration: Duration) -> Sleep {
        Sleep {
            deadline: Instant::now() + duration,
            _phantom: PhantomData,
        }
    }

    pub fn sleep_until(deadline: Instant) -> Sleep {
        Sleep {
            deadline,
            _phantom: PhantomData,
        }
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn is_elapsed(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        let this = self.get_mut();
        let now = Instant::now();
        if now >= this.deadline {
            Poll::Ready(())
        } else {
            let task = unsafe { &mut *(cx.waker().data() as *mut RawTask) };
            // Safe: a polled task has always been spawned, so the executor is set.
            let raw_executor = task
                .get_executor()
                .expect("polled task has no executor")
                .raw();
            let pinned_task = unsafe { Pin::new_unchecked(task) };
            let executor = unsafe { &*raw_executor };
            executor.task_sleep_until(pinned_task, this.deadline);
            Poll::Pending
        }
    }
}
