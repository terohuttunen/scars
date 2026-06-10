//! Future combinators for the built-in executor.
//!
//! These poll their children with the *parent* task's [`Context`], so the
//! shared task waker drives re-scheduling. They rely on the executor's
//! idempotent task-wake (a child may self-requeue the task, and several
//! children may register wake sources at once); see the executor's
//! `resume_task` / `task_sleep_until` / `poll_ready_tasks`. A dropped child
//! (such as the losing arm of `select!`) leaves a transient pending-ready
//! entry, drained on the next poll, or a stale timer bounded by the per-task
//! merge and cleared when the task completes.
//!
//! `join!`/`select!` cover arity 2 and 3; the [`join`]/[`join3`]/[`select`]/
//! [`select3`] functions are the explicit forms.

use crate::task::sleep::Sleep;
use crate::time::{Duration, Instant};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

/// Result of [`select`]: whichever branch finished first.
pub enum Either<A, B> {
    First(A),
    Second(B),
}

/// Result of [`select3`].
pub enum Either3<A, B, C> {
    First(A),
    Second(B),
    Third(C),
}

/// Error returned by [`timeout`] / [`with_deadline`] when the deadline elapses
/// before the future completes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Elapsed;

// ----- join -----------------------------------------------------------------

#[pin_project::pin_project]
pub struct Join<A: Future, B: Future> {
    #[pin]
    a: A,
    a_out: Option<A::Output>,
    #[pin]
    b: B,
    b_out: Option<B::Output>,
}

/// Poll both futures concurrently; complete with both outputs.
pub fn join<A: Future, B: Future>(a: A, b: B) -> Join<A, B> {
    Join {
        a,
        a_out: None,
        b,
        b_out: None,
    }
}

impl<A: Future, B: Future> Future for Join<A, B> {
    type Output = (A::Output, B::Output);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if this.a_out.is_none() {
            if let Poll::Ready(v) = this.a.poll(cx) {
                *this.a_out = Some(v);
            }
        }
        if this.b_out.is_none() {
            if let Poll::Ready(v) = this.b.poll(cx) {
                *this.b_out = Some(v);
            }
        }
        if this.a_out.is_some() && this.b_out.is_some() {
            Poll::Ready((this.a_out.take().unwrap(), this.b_out.take().unwrap()))
        } else {
            Poll::Pending
        }
    }
}

#[pin_project::pin_project]
pub struct Join3<A: Future, B: Future, C: Future> {
    #[pin]
    a: A,
    a_out: Option<A::Output>,
    #[pin]
    b: B,
    b_out: Option<B::Output>,
    #[pin]
    c: C,
    c_out: Option<C::Output>,
}

pub fn join3<A: Future, B: Future, C: Future>(a: A, b: B, c: C) -> Join3<A, B, C> {
    Join3 {
        a,
        a_out: None,
        b,
        b_out: None,
        c,
        c_out: None,
    }
}

impl<A: Future, B: Future, C: Future> Future for Join3<A, B, C> {
    type Output = (A::Output, B::Output, C::Output);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if this.a_out.is_none() {
            if let Poll::Ready(v) = this.a.poll(cx) {
                *this.a_out = Some(v);
            }
        }
        if this.b_out.is_none() {
            if let Poll::Ready(v) = this.b.poll(cx) {
                *this.b_out = Some(v);
            }
        }
        if this.c_out.is_none() {
            if let Poll::Ready(v) = this.c.poll(cx) {
                *this.c_out = Some(v);
            }
        }
        if this.a_out.is_some() && this.b_out.is_some() && this.c_out.is_some() {
            Poll::Ready((
                this.a_out.take().unwrap(),
                this.b_out.take().unwrap(),
                this.c_out.take().unwrap(),
            ))
        } else {
            Poll::Pending
        }
    }
}

// ----- select ---------------------------------------------------------------

#[pin_project::pin_project]
pub struct Select<A: Future, B: Future> {
    #[pin]
    a: A,
    #[pin]
    b: B,
}

/// Poll both futures; complete with the first to finish (the other is dropped).
pub fn select<A: Future, B: Future>(a: A, b: B) -> Select<A, B> {
    Select { a, b }
}

impl<A: Future, B: Future> Future for Select<A, B> {
    type Output = Either<A::Output, B::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if let Poll::Ready(v) = this.a.poll(cx) {
            return Poll::Ready(Either::First(v));
        }
        if let Poll::Ready(v) = this.b.poll(cx) {
            return Poll::Ready(Either::Second(v));
        }
        Poll::Pending
    }
}

#[pin_project::pin_project]
pub struct Select3<A: Future, B: Future, C: Future> {
    #[pin]
    a: A,
    #[pin]
    b: B,
    #[pin]
    c: C,
}

pub fn select3<A: Future, B: Future, C: Future>(a: A, b: B, c: C) -> Select3<A, B, C> {
    Select3 { a, b, c }
}

impl<A: Future, B: Future, C: Future> Future for Select3<A, B, C> {
    type Output = Either3<A::Output, B::Output, C::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if let Poll::Ready(v) = this.a.poll(cx) {
            return Poll::Ready(Either3::First(v));
        }
        if let Poll::Ready(v) = this.b.poll(cx) {
            return Poll::Ready(Either3::Second(v));
        }
        if let Poll::Ready(v) = this.c.poll(cx) {
            return Poll::Ready(Either3::Third(v));
        }
        Poll::Pending
    }
}

// ----- timeout --------------------------------------------------------------

#[pin_project::pin_project]
pub struct Timeout<F: Future> {
    #[pin]
    fut: F,
    #[pin]
    sleep: Sleep,
}

/// Complete with `Ok(fut output)` if `fut` finishes within `duration`, else
/// `Err(Elapsed)`.
pub fn timeout<F: Future>(duration: Duration, fut: F) -> Timeout<F> {
    Timeout {
        fut,
        sleep: Sleep::sleep(duration),
    }
}

/// Like [`timeout`] but bounded by an absolute `deadline`.
pub fn with_deadline<F: Future>(deadline: Instant, fut: F) -> Timeout<F> {
    Timeout {
        fut,
        sleep: Sleep::sleep_until(deadline),
    }
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, Elapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if let Poll::Ready(v) = this.fut.poll(cx) {
            return Poll::Ready(Ok(v));
        }
        if this.sleep.poll(cx).is_ready() {
            return Poll::Ready(Err(Elapsed));
        }
        Poll::Pending
    }
}

// ----- macros ---------------------------------------------------------------

/// Poll the given futures concurrently and complete with a tuple of all
/// outputs. Supports 2 or 3 futures.
#[macro_export]
macro_rules! join {
    ($a:expr, $b:expr $(,)?) => {
        $crate::task::combinators::join($a, $b)
    };
    ($a:expr, $b:expr, $c:expr $(,)?) => {
        $crate::task::combinators::join3($a, $b, $c)
    };
}

/// Poll the given futures and complete with the first to finish, as an
/// [`Either`]/[`Either3`]. Supports 2 or 3 futures.
#[macro_export]
macro_rules! select {
    ($a:expr, $b:expr $(,)?) => {
        $crate::task::combinators::select($a, $b)
    };
    ($a:expr, $b:expr, $c:expr $(,)?) => {
        $crate::task::combinators::select3($a, $b, $c)
    };
}
