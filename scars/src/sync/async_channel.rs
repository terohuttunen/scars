//! Bounded async channel for tasks on a single executor.
//!
//! A classic bounded buffer: the ring buffer lives in an
//! [`AsyncMutex`](crate::sync::AsyncMutex) and a paired
//! [`AsyncCondvar`](crate::sync::AsyncCondvar) covers the full/empty wait. A
//! `send` to a full channel or `recv` from an empty one waits on the condvar; a
//! successful operation notifies it so the peer re-checks. Access contention is
//! handled by the mutex (the task waits for the lock), never by panicking.
//!
//! An `AsyncChannel` is a plain `static`. For cross-executor / thread /
//! interrupt messaging use the synchronous [`Channel`](crate::sync::channel).

use crate::sync::async_condvar::AsyncCondvar;
use crate::sync::async_mutex::AsyncMutex;
use crate::sync::fifo::FIFO;

pub struct AsyncChannel<T, const CAPACITY: usize> {
    buf: AsyncMutex<FIFO<T, CAPACITY>>,
    cond: AsyncCondvar,
}

impl<T, const CAPACITY: usize> AsyncChannel<T, CAPACITY> {
    pub const fn new() -> AsyncChannel<T, CAPACITY> {
        AsyncChannel {
            buf: AsyncMutex::new(FIFO::new()),
            cond: AsyncCondvar::new(),
        }
    }

    /// Send `item`, waiting while the channel is full.
    pub async fn send(&self, item: T) {
        let guard = self.buf.lock().await;
        let mut guard = self.cond.wait_while(guard, |fifo| fifo.is_full()).await;
        guard.push(item);
        drop(guard);
        self.cond.notify_all();
    }

    /// Receive an item, waiting while the channel is empty.
    pub async fn recv(&self) -> T {
        let guard = self.buf.lock().await;
        let mut guard = self.cond.wait_while(guard, |fifo| fifo.is_empty()).await;
        let item = guard.pop().expect("channel non-empty after wait");
        drop(guard);
        self.cond.notify_all();
        item
    }

    /// Non-blocking send; returns the item back if the lock is held or the
    /// channel is full.
    pub fn try_send(&self, item: T) -> Result<(), T> {
        let mut guard = match self.buf.try_lock() {
            Some(guard) => guard,
            None => return Err(item),
        };
        if guard.is_full() {
            return Err(item);
        }
        guard.push(item);
        drop(guard);
        self.cond.notify_all();
        Ok(())
    }

    /// Non-blocking receive; returns `None` if the lock is held or the channel
    /// is empty.
    pub fn try_recv(&self) -> Option<T> {
        let mut guard = self.buf.try_lock()?;
        let item = guard.pop();
        if item.is_some() {
            drop(guard);
            self.cond.notify_all();
        }
        item
    }
}
