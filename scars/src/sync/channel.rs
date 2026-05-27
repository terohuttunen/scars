use crate::kernel::Priority;
use crate::kernel::hal::CoreId;
use crate::sync::atomic::{AtomicBool, Ordering};
use crate::sync::{
    BarrierResult, CoreCeilingLock, CorePreemptLock, NestingLock, Notify, Protected, TimedOut,
};
use crate::time::Instant;
use core::mem::MaybeUninit;

#[macro_export]
macro_rules! make_channel {
    ($ty:path, $size:expr, $prio:expr $(, core = $core:expr)?) => {{
        static mut CHANNEL: $crate::sync::channel::CeilingChannel<
            $ty,
            { $size },
            { $prio },
            { $crate::make_channel!(@core $($core)?) },
        > = $crate::sync::channel::CeilingChannel::new();

        unsafe { CHANNEL.split() }
    }};
    ($ty:path, $size:expr) => {{
        static mut CHANNEL: $crate::sync::channel::Channel<$ty, { $size }> =
            $crate::sync::channel::Channel::new();

        unsafe { CHANNEL.split() }
    }};
    (@core) => {$crate::CoreId::DEFAULT};
    (@core $core:expr) => { $core };
}

pub use make_channel;

pub type CeilingChannel<
    T,
    const CAPACITY: usize,
    const CEILING: Priority,
    const CORE: CoreId = { CoreId::DEFAULT },
> = LockedChannel<T, CAPACITY, CoreCeilingLock<CEILING, CORE>>;
pub type CeilingSender<
    T,
    const CAPACITY: usize,
    const CEILING: Priority,
    const CORE: CoreId = { CoreId::DEFAULT },
> = LockedSender<T, CAPACITY, CoreCeilingLock<CEILING, CORE>>;
pub type CeilingReceiver<
    T,
    const CAPACITY: usize,
    const CEILING: Priority,
    const CORE: CoreId = { CoreId::DEFAULT },
> = LockedReceiver<T, CAPACITY, CoreCeilingLock<CEILING, CORE>>;

pub type Channel<T, const CAPACITY: usize, const CORE: CoreId = { CoreId::DEFAULT }> =
    LockedChannel<T, CAPACITY, CorePreemptLock<CORE>>;
pub type Sender<T, const CAPACITY: usize, const CORE: CoreId = { CoreId::DEFAULT }> =
    LockedSender<T, CAPACITY, CorePreemptLock<CORE>>;
pub type Receiver<T, const CAPACITY: usize, const CORE: CoreId = { CoreId::DEFAULT }> =
    LockedReceiver<T, CAPACITY, CorePreemptLock<CORE>>;

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

pub struct LockedChannel<T, const CAPACITY: usize, L: NestingLock> {
    receiver_acquired: AtomicBool,
    fifo: Protected<FIFO<T, CAPACITY>, L>,
    senders: Notify<L>,
    receivers: Notify<L>,
}

impl<T, const CAPACITY: usize, L: NestingLock> LockedChannel<T, CAPACITY, L> {
    pub const fn new() -> LockedChannel<T, CAPACITY, L> {
        LockedChannel {
            receiver_acquired: AtomicBool::new(false),
            fifo: Protected::new(FIFO::new()),
            senders: Notify::new(),
            receivers: Notify::new(),
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.fifo.with(|_, fifo| match fifo.pop() {
            Some(item) => {
                self.senders.notify_one();
                Ok(item)
            }
            None => Err(TryRecvError::Empty),
        })
    }

    pub fn recv(&'static self) -> T {
        self.fifo.with_barrier(|key, fifo| match fifo.pop() {
            Some(v) => {
                self.senders.notify_one();
                BarrierResult::Done(v)
            }
            None => BarrierResult::Wait(self.receivers.arm(key)),
        })
    }

    pub fn send(&'static self, item: T) {
        let mut item = Some(item);
        self.fifo.with_barrier(|key, fifo| {
            if !fifo.is_full() {
                fifo.push(item.take().unwrap());
                self.receivers.notify_one();
                BarrierResult::Done(())
            } else {
                BarrierResult::Wait(self.senders.arm(key))
            }
        });
    }

    /// Like [`recv`](Self::recv), but bounded by `deadline`.
    /// Returns `Err(TimedOut)` if the deadline elapses before an
    /// item arrives.
    pub fn recv_until(&'static self, deadline: Instant) -> Result<T, TimedOut> {
        self.fifo
            .with_barrier_until(deadline, |key, fifo| match fifo.pop() {
                Some(v) => {
                    self.senders.notify_one();
                    BarrierResult::Done(v)
                }
                None => BarrierResult::Wait(self.receivers.arm(key)),
            })
    }

    /// Like [`send`](Self::send), but bounded by `deadline`.
    /// Returns `Err(TimedOut)` if the deadline elapses before a
    /// slot becomes free. On timeout the item is dropped.
    pub fn send_until(&'static self, item: T, deadline: Instant) -> Result<(), TimedOut> {
        let mut item = Some(item);
        self.fifo.with_barrier_until(deadline, |key, fifo| {
            if !fifo.is_full() {
                fifo.push(item.take().unwrap());
                self.receivers.notify_one();
                BarrierResult::Done(())
            } else {
                BarrierResult::Wait(self.senders.arm(key))
            }
        })
    }

    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        let mut item = Some(item);
        self.fifo.with(|_, fifo| {
            if fifo.is_full() {
                Err(TrySendError::Full(item.take().unwrap()))
            } else {
                fifo.push(item.take().unwrap());
                self.receivers.notify_one();
                Ok(())
            }
        })
    }

    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    pub fn free(&self) -> usize {
        self.fifo.with(|_, fifo| fifo.free())
    }

    pub fn used(&self) -> usize {
        self.fifo.with(|_, fifo| fifo.used())
    }

    pub fn receiver(&'static self) -> LockedReceiver<T, CAPACITY, L> {
        match self.receiver_acquired.compare_exchange(
            false,
            true,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => LockedReceiver { channel: self },
            Err(_) => panic!("Receiver already acquired"),
        }
    }

    pub fn sender(&'static self) -> LockedSender<T, CAPACITY, L> {
        LockedSender { channel: self }
    }

    pub fn split(
        &'static mut self,
    ) -> (LockedSender<T, CAPACITY, L>, LockedReceiver<T, CAPACITY, L>) {
        (self.sender(), self.receiver())
    }
}

pub struct LockedSender<T: 'static, const CAPACITY: usize, L: NestingLock + 'static> {
    channel: &'static LockedChannel<T, CAPACITY, L>,
}

impl<T, const CAPACITY: usize, L: NestingLock> LockedSender<T, CAPACITY, L> {
    pub fn send(&self, t: T) {
        self.channel.send(t)
    }

    pub fn send_until(&self, item: T, deadline: Instant) -> Result<(), TimedOut> {
        self.channel.send_until(item, deadline)
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

unsafe impl<T: Send, const CAPACITY: usize, L: NestingLock> Send for LockedSender<T, CAPACITY, L> {}

unsafe impl<T: Send, const CAPACITY: usize, L: NestingLock> Sync for LockedSender<T, CAPACITY, L> {}

impl<T, const CAPACITY: usize, L: NestingLock> Clone for LockedSender<T, CAPACITY, L> {
    fn clone(&self) -> Self {
        LockedSender {
            channel: self.channel,
        }
    }
}

pub struct LockedReceiver<T: 'static, const CAPACITY: usize, L: NestingLock + 'static> {
    channel: &'static LockedChannel<T, CAPACITY, L>,
}

impl<T, const CAPACITY: usize, L: NestingLock> LockedReceiver<T, CAPACITY, L> {
    pub fn recv(&self) -> T {
        self.channel.recv()
    }

    pub fn recv_until(&self, deadline: Instant) -> Result<T, TimedOut> {
        self.channel.recv_until(deadline)
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

impl<T, const CAPACITY: usize, L: NestingLock> Drop for LockedReceiver<T, CAPACITY, L> {
    fn drop(&mut self) {
        self.channel
            .receiver_acquired
            .store(false, Ordering::SeqCst);
    }
}

unsafe impl<T: Send, const CAPACITY: usize, L: NestingLock> Send
    for LockedReceiver<T, CAPACITY, L>
{
}
