use crate::sync::channel::FIFO;
use crate::sync::{AsyncCondvar, AsyncMutex};

pub struct AsyncChannel<T, const CAPACITY: usize> {
    receiver_acquired: bool,
    fifo: AsyncMutex<FIFO<T, CAPACITY>>,
    receivers: AsyncCondvar,
    senders: AsyncCondvar,
}

impl<T, const CAPACITY: usize> AsyncChannel<T, CAPACITY> {
    pub const fn new() -> AsyncChannel<T, CAPACITY> {
        AsyncChannel {
            receiver_acquired: false,
            fifo: AsyncMutex::new(FIFO::new()),
            receivers: AsyncCondvar::new(),
            senders: AsyncCondvar::new(),
        }
    }

    pub async fn recv(&self) -> T {
        let mut fifo_guard = self.fifo.lock().await;
        self.receivers
            .wait_while(fifo_guard, |fifo| fifo.is_empty())
            .await;
        let item = fifo_guard.pop().unwrap();
        self.senders.notify_one();
        item
    }

    pub async fn send(&self, item: T) {
        let mut fifo_guard = self.fifo.lock().await;
        self.senders
            .wait_while(fifo_guard, |fifo| fifo.is_full())
            .await;
        let _ = fifo_guard.push(item);
        self.receivers.notify_one();
    }
}
