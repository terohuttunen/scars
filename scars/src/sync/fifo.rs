//! Fixed-capacity single-threaded ring buffer shared by the blocking
//! [`Channel`](crate::sync::channel) and the async
//! [`AsyncChannel`](crate::sync::async_channel). Mutual exclusion is the
//! caller's responsibility (a `Protected` for the blocking channel, the
//! executor's cooperative poll for the async one).

use core::mem::MaybeUninit;

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
