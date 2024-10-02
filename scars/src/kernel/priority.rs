use core::sync::atomic::{AtomicI16, AtomicU16, AtomicU32, Ordering};

pub type ThreadPriority = u8;
pub type InterruptPriority = u8;

const INTERRUPT_BIT: i16 = 1 << (i16::BITS - 2);
pub const INVALID_ANY_PRIORITY: AnyPriority = -1;
pub const INVALID_PRIORITY: Priority = Priority::any(INVALID_ANY_PRIORITY);

pub type AnyPriority = i16;

pub const fn any_thread_priority(prio: ThreadPriority) -> AnyPriority {
    prio as AnyPriority
}

pub const fn any_interrupt_priority(prio: InterruptPriority) -> AnyPriority {
    prio as AnyPriority | INTERRUPT_BIT
}

#[derive(Copy, Clone, Debug, Ord, PartialOrd, PartialEq, Eq)]
#[repr(transparent)]
pub struct Priority(AnyPriority);

impl Priority {
    pub const MIN_THREAD_PRIORITY: Priority = Priority::thread_priority(ThreadPriority::MIN);
    pub const MAX_THREAD_PRIORITY: Priority = Priority::thread_priority(ThreadPriority::MAX);
    pub const MIN_INTERRUPT_PRIORITY: Priority =
        Priority::interrupt_priority(InterruptPriority::MIN);
    pub const MAX_INTERRUPT_PRIORITY: Priority =
        Priority::interrupt_priority(InterruptPriority::MAX);

    pub const fn any(prio: AnyPriority) -> Priority {
        Priority(prio)
    }

    pub const fn thread_priority(prio: ThreadPriority) -> Priority {
        Priority(prio as i16)
    }

    pub const fn any_thread_priority(prio: ThreadPriority) -> AnyPriority {
        prio as AnyPriority
    }

    pub const fn interrupt_priority(prio: InterruptPriority) -> Priority {
        Priority(prio as i16 | INTERRUPT_BIT)
    }

    pub const fn any_interrupt_priority(prio: InterruptPriority) -> AnyPriority {
        prio as AnyPriority | INTERRUPT_BIT
    }

    pub const fn is_interrupt(&self) -> bool {
        (self.0 & INTERRUPT_BIT) != 0
    }

    pub const fn is_thread(&self) -> bool {
        (self.0 & INTERRUPT_BIT) == 0
    }

    pub const fn is_valid(&self) -> bool {
        self.0 >= 0
    }

    pub const fn get_value(&self) -> u8 {
        (self.0 & !INTERRUPT_BIT) as u8
    }

    pub const fn into_any(self) -> AnyPriority {
        self.0
    }

    pub const fn succ(self) -> Priority {
        if self.is_valid() && self.get_value() <= Self::MAX_THREAD_PRIORITY.get_value() {
            Priority(self.0 + 1)
        } else {
            Priority(self.0)
        }
    }

    pub const fn max(self, other: Priority) -> Priority {
        if self.0 > other.0 {
            self
        } else {
            other
        }
    }
}

impl core::fmt::Display for Priority {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_thread() {
            write!(f, "T{}", self.0)
        } else {
            write!(f, "I{}", self.get_value())
        }
    }
}

pub struct AtomicPriority(AtomicI16);

impl AtomicPriority {
    pub const fn new(prio: Priority) -> AtomicPriority {
        AtomicPriority::any(prio.0)
    }

    pub const fn any(prio: AnyPriority) -> AtomicPriority {
        AtomicPriority(AtomicI16::new(prio))
    }

    pub const fn thread_priority(prio: ThreadPriority) -> AtomicPriority {
        AtomicPriority(AtomicI16::new(prio as i16))
    }

    pub const fn interrupt_priority(prio: InterruptPriority) -> AtomicPriority {
        AtomicPriority(AtomicI16::new(prio as i16 | INTERRUPT_BIT))
    }

    pub fn is_interrupt(&self) -> bool {
        (self.0.load(Ordering::Relaxed) & INTERRUPT_BIT) != 0
    }

    pub fn is_thread(&self) -> bool {
        (self.0.load(Ordering::Relaxed) & INTERRUPT_BIT) == 0
    }

    pub fn load(&self, ordering: Ordering) -> Priority {
        Priority(self.0.load(ordering))
    }

    pub fn store(&self, prio: Priority, ordering: Ordering) {
        self.0.store(prio.0, ordering)
    }

    pub fn swap(&self, prio: Priority, ordering: Ordering) -> Priority {
        Priority::any(self.0.swap(prio.0, ordering))
    }

    pub fn compare_exchange(
        &self,
        current: Priority,
        new: Priority,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Priority, Priority> {
        self.0
            .compare_exchange(current.into_any(), new.into_any(), success, failure)
            .map(|x| Priority::any(x))
            .map_err(|e| Priority::any(e))
    }

    pub fn fetch_update<F>(
        &self,
        set_order: Ordering,
        fetch_order: Ordering,
        mut f: F,
    ) -> Result<Priority, Priority>
    where
        F: FnMut(Priority) -> Option<Priority>,
    {
        self.0
            .fetch_update(set_order, fetch_order, |prio| {
                f(Priority::any(prio)).map(|x| x.into_any())
            })
            .map(|x| Priority::any(x))
            .map_err(|e| Priority::any(e))
    }
}

const fn from_pair(pair: (Priority, Priority)) -> u32 {
    unsafe { core::mem::transmute::<(Priority, Priority), u32>(pair) }
}

const fn to_pair(value: u32) -> (Priority, Priority) {
    unsafe { core::mem::transmute::<u32, (Priority, Priority)>(value) }
}

#[repr(transparent)]
pub struct AtomicPriorityPair(AtomicU32);

impl AtomicPriorityPair {
    pub const fn new(pair: (Priority, Priority)) -> AtomicPriorityPair {
        AtomicPriorityPair(AtomicU32::new(from_pair(pair)))
    }

    pub fn load(&self, ordering: Ordering) -> (Priority, Priority) {
        let value = self.0.load(ordering);
        to_pair(value)
    }

    pub fn store(&self, pair: (Priority, Priority), ordering: Ordering) {
        let value = from_pair(pair);
        self.0.store(value, ordering)
    }

    pub fn swap(&self, pair: (Priority, Priority), ordering: Ordering) -> (Priority, Priority) {
        let value = from_pair(pair);
        to_pair(self.0.swap(value, ordering))
    }

    pub fn compare_exchange(
        &self,
        current: (Priority, Priority),
        new: (Priority, Priority),
        success: Ordering,
        failure: Ordering,
    ) -> Result<(Priority, Priority), (Priority, Priority)> {
        self.0
            .compare_exchange(from_pair(current), from_pair(new), success, failure)
            .map(|x| to_pair(x))
            .map_err(|e| to_pair(e))
    }

    pub fn fetch_update<F>(
        &self,
        set_order: Ordering,
        fetch_order: Ordering,
        mut f: F,
    ) -> Result<(Priority, Priority), (Priority, Priority)>
    where
        F: FnMut((Priority, Priority)) -> Option<(Priority, Priority)>,
    {
        self.0
            .fetch_update(set_order, fetch_order, |value| {
                f(to_pair(value)).map(|pair| from_pair(pair))
            })
            .map(|x| to_pair(x))
            .map_err(|e| to_pair(e))
    }
}
