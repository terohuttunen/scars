use crate::kernel::AnyPriority;
use crate::sync::condvar::Condvar;
use crate::sync::mutex::Mutex;
use core::mem::MaybeUninit;

#[macro_export]
macro_rules! make_channel {
    ($ty:path, $size:expr, $prio:expr) => {{
        static mut CHANNEL: $crate::sync::channel::Channel<$ty, { $size }, { $prio }> =
            $crate::sync::channel::Channel::new();

        unsafe { CHANNEL.split() }
    }};
}

pub struct FIFO<T, const CAPACITY: usize> {
    // Where new data can be written (unless full)
    head: usize,

    // Where data can be read from (unless empty)
    tail: usize,

    // how many entries out of CAPACITY has been used
    used: usize,

    fifo: [MaybeUninit<T>; CAPACITY],
}

// Function to push into slice, in order to avoid monomorphization
// with respect to CAPACITY.
#[inline(never)]
fn push_to_slice<T>(
    head: &mut usize,
    used: &mut usize,
    fifo: &mut [MaybeUninit<T>],
    item: T,
) -> bool {
    // Make sure there is a free slot; if not, return `false`.
    if *used == fifo.len() {
        return false;
    }

    // Write value to a free slot pointed by `head`
    let _ = fifo[*head].write(item);

    // Advance head-pointer with a wrap-around
    *head = (*head + 1) % fifo.len();

    *used += 1;

    true
}

// Function to pop from slice, in order to avoid monomorphization
// with respect to CAPACITY.
#[inline(never)]
fn pop_from_slice<T>(tail: &mut usize, used: &mut usize, fifo: &[MaybeUninit<T>]) -> Option<T> {
    if *used == 0 {
        return None;
    }

    let item = unsafe { fifo[*tail].assume_init_read() };

    *tail = (*tail + 1) % fifo.len();

    *used -= 1;

    Some(item)
}

impl<T, const CAPACITY: usize> FIFO<T, CAPACITY> {
    pub const fn new() -> FIFO<T, CAPACITY> {
        FIFO {
            head: 0,
            tail: 0,
            used: 0,
            fifo: unsafe { MaybeUninit::uninit().assume_init() },
        }
    }

    #[inline]
    pub fn push(&mut self, item: T) -> bool {
        push_to_slice(&mut self.head, &mut self.used, &mut self.fifo[..], item)
    }

    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        pop_from_slice(&mut self.tail, &mut self.used, &self.fifo[..])
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        CAPACITY
    }

    #[inline]
    pub fn used(&self) -> usize {
        self.used
    }

    #[inline]
    pub fn free(&self) -> usize {
        CAPACITY - self.used
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    #[inline]
    pub fn is_full(&self) -> bool {
        self.used == CAPACITY
    }
}

impl<T, const CAPACITY: usize> Drop for FIFO<T, CAPACITY> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TryRecvError {
    Empty,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TrySendError<T> {
    Full(T),
}

pub struct Channel<T, const CAPACITY: usize, const CEILING: AnyPriority> {
    fifo: Mutex<FIFO<T, CAPACITY>, CEILING>,
    receivers: Condvar<CEILING>,
    senders: Condvar<CEILING>,
}

impl<T, const CAPACITY: usize, const CEILING: AnyPriority> Channel<T, CAPACITY, CEILING> {
    pub const fn new() -> Channel<T, CAPACITY, CEILING> {
                Channel {
            fifo: Mutex::new(FIFO::new()),
            receivers: Condvar::new(),
            senders: Condvar::new(),
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut fifo_guard = self.fifo.lock();

        if fifo_guard.is_empty() {
            return Err(TryRecvError::Empty);
        }

        let item = fifo_guard.pop().unwrap();

        // Drop FIFO guard before activating senders
        drop(fifo_guard);

        self.senders.notify_one();

        Ok(item)
    }

    pub fn recv(&self) -> T {
        let fifo_guard = self.fifo.lock();

        // Wait file FIFO is empty
        let mut fifo_guard = self
            .receivers
            .wait_while(fifo_guard, |fifo| fifo.is_empty());

        // FIFO is locked and cannot be empty, pop one item
        let item = fifo_guard.pop().unwrap();

        // Drop FIFO guard before activating senders
        drop(fifo_guard);

        // Notify blocked senders that there is room in the FIFO
        self.senders.notify_one();

        item
    }

    pub fn send(&self, item: T) {
        let fifo_guard = self.fifo.lock();

        // Wait while FIFO is full
        let mut fifo_guard = self.senders.wait_while(fifo_guard, |fifo| fifo.is_full());

        fifo_guard.push(item);

        // Drop FIFO guard before activating receivers
        drop(fifo_guard);

        self.receivers.notify_one();
    }

    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        let mut fifo_guard = self.fifo.lock();

        if fifo_guard.is_full() {
            return Err(TrySendError::Full(item));
        }

        fifo_guard.push(item);

        // Drop FIFO guard before activating receivers
        drop(fifo_guard);

        self.receivers.notify_one();

        Ok(())
    }

    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    pub fn free(&self) -> usize {
        self.fifo.lock().free()
    }

    pub fn used(&self) -> usize {
        self.fifo.lock().used()
    }

    pub const fn split(
        &'static mut self,
    ) -> (Sender<T, CAPACITY, CEILING>, Receiver<T, CAPACITY, CEILING>) {
        (Sender { channel: self }, Receiver { channel: self })
    }
}

#[derive(Clone)]
pub struct Sender<T: 'static, const CAPACITY: usize, const CEILING: AnyPriority> {
    channel: &'static Channel<T, CAPACITY, CEILING>,
}

impl<T, const CAPACITY: usize, const CEILING: AnyPriority> Sender<T, CAPACITY, CEILING> {
    pub fn send(&self, t: T) {
        self.channel.send(t)
    }

    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        self.channel.try_send(item)
    }

    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    pub fn free(&self) -> usize {
        self.channel.free()
    }

    pub fn used(&self) -> usize {
        self.channel.used()
    }
}

unsafe impl<T: Send, const CAPACITY: usize, const CEILING: AnyPriority> Send
    for Sender<T, CAPACITY, CEILING>
{
}

unsafe impl<T: Send, const CAPACITY: usize, const CEILING: AnyPriority> Sync
    for Sender<T, CAPACITY, CEILING>
{
}

pub struct Receiver<T: 'static, const CAPACITY: usize, const CEILING: AnyPriority> {
    channel: &'static Channel<T, CAPACITY, CEILING>,
}

impl<T, const CAPACITY: usize, const CEILING: AnyPriority> Receiver<T, CAPACITY, CEILING> {
    pub fn recv(&self) -> T {
        self.channel.recv()
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.channel.try_recv()
    }

    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    pub fn free(&self) -> usize {
        self.channel.free()
    }

    pub fn used(&self) -> usize {
        self.channel.used()
    }
}

unsafe impl<T: Send, const CAPACITY: usize, const CEILING: AnyPriority> Send
    for Receiver<T, CAPACITY, CEILING>
{
}
