use core::marker::ConstParamTy;
use core::sync::atomic::{AtomicI16, AtomicU16, AtomicU32, Ordering};

pub type ThreadPriority = u8;
pub type InterruptPriority = u8;

pub type AnyPriority = i16;

const INTERRUPT_BIT: i16 = 1 << (i16::BITS - 2);
pub const INVALID_PRIORITY: i16 = -1;

const fn maxu8(left: u8, right: u8) -> u8 {
    if left > right {
        left
    } else {
        right
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ConstParamTy)]
#[repr(u8)]
pub enum Priority {
    Thread(ThreadPriority),
    Interrupt(InterruptPriority),
}

impl Priority {
    pub const MIN: Priority = Priority::Thread(ThreadPriority::MIN);
    pub const MAX: Priority = Priority::Interrupt(InterruptPriority::MAX);
    pub const THREAD_MIN: Priority = Priority::Thread(ThreadPriority::MIN);
    pub const THREAD_MAX: Priority = Priority::Thread(ThreadPriority::MAX);
    pub const INTERRUPT_MIN: Priority = Priority::Interrupt(InterruptPriority::MIN);
    pub const INTERRUPT_MAX: Priority = Priority::Interrupt(InterruptPriority::MAX);

    pub const fn thread(prio: ThreadPriority) -> Priority {
        Priority::Thread(prio)
    }

    pub const fn interrupt(prio: InterruptPriority) -> Priority {
        Priority::Interrupt(prio)
    }

    pub const fn is_interrupt(&self) -> bool {
        matches!(self, Priority::Interrupt(_))
    }

    pub const fn is_thread(&self) -> bool {
        matches!(self, Priority::Thread(_))
    }

    pub const fn get_value(&self) -> u8 {
        match self {
            Priority::Thread(prio) => *prio,
            Priority::Interrupt(prio) => *prio,
        }
    }

    pub const fn succ(self) -> Priority {
        match self {
            Priority::Thread(prio) => {
                if prio < ThreadPriority::MAX {
                    Priority::Thread(prio + 1)
                } else {
                    Priority::INTERRUPT_MIN
                }
            }
            Priority::Interrupt(prio) => {
                if prio < InterruptPriority::MAX {
                    Priority::Interrupt(prio + 1)
                } else {
                    Priority::Interrupt(prio)
                }
            }
        }
    }

    pub const fn max(self, other: Priority) -> Priority {
        match (self, other) {
            (Priority::Thread(p1), Priority::Thread(p2)) => Priority::Thread(maxu8(p1, p2)),
            (Priority::Interrupt(p1), Priority::Interrupt(p2)) => {
                Priority::Interrupt(maxu8(p1, p2))
            }
            (Priority::Thread(_), Priority::Interrupt(_)) => other,
            (Priority::Interrupt(_), Priority::Thread(_)) => self,
        }
    }

    pub const fn max_valid(self, other: PriorityStatus) -> Priority {
        let other = match other.priority() {
            Some(prio) => prio,
            None => return self,
        };

        match (self, other) {
            (Priority::Thread(p1), Priority::Thread(p2)) => Priority::Thread(maxu8(p1, p2)),
            (Priority::Interrupt(p1), Priority::Interrupt(p2)) => {
                Priority::Interrupt(maxu8(p1, p2))
            }
            (Priority::Thread(_), Priority::Interrupt(_)) => other,
            (Priority::Interrupt(_), Priority::Thread(_)) => self,
        }
    }

    pub const fn into_any(self) -> AnyPriority {
        match self {
            Priority::Thread(prio) => prio as i16,
            Priority::Interrupt(prio) => prio as i16 | INTERRUPT_BIT,
        }
    }

    pub const fn from_any(value: AnyPriority) -> Priority {
        if (value & INTERRUPT_BIT) != 0 {
            Priority::Interrupt((value & !INTERRUPT_BIT) as InterruptPriority)
        } else {
            Priority::Thread(value as ThreadPriority)
        }
    }
}

impl core::fmt::Display for Priority {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Priority::Thread(prio) => write!(f, "T{}", prio),
            Priority::Interrupt(prio) => write!(f, "I{}", prio),
        }
    }
}

impl core::cmp::PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        match (self, other) {
            (Priority::Thread(p1), Priority::Thread(p2)) => p1.partial_cmp(p2),
            (Priority::Interrupt(p1), Priority::Interrupt(p2)) => p1.partial_cmp(p2),
            (Priority::Thread(_), Priority::Interrupt(_)) => Some(core::cmp::Ordering::Less),
            (Priority::Interrupt(_), Priority::Thread(_)) => Some(core::cmp::Ordering::Greater),
        }
    }
}

impl core::cmp::Ord for Priority {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        match (self, other) {
            (Priority::Thread(p1), Priority::Thread(p2)) => p1.cmp(p2),
            (Priority::Interrupt(p1), Priority::Interrupt(p2)) => p1.cmp(p2),
            (Priority::Thread(_), Priority::Interrupt(_)) => core::cmp::Ordering::Less,
            (Priority::Interrupt(_), Priority::Thread(_)) => core::cmp::Ordering::Greater,
        }
    }
}

/// Like `Priority`, but with an additional `Invalid` variant.
#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum PriorityStatus {
    Invalid,
    Thread(ThreadPriority),
    Interrupt(InterruptPriority),
}

impl PriorityStatus {
    pub const fn invalid() -> PriorityStatus {
        PriorityStatus::Invalid
    }

    pub const fn thread(prio: ThreadPriority) -> PriorityStatus {
        PriorityStatus::Thread(prio)
    }

    pub const fn interrupt(prio: InterruptPriority) -> PriorityStatus {
        PriorityStatus::Interrupt(prio)
    }

    pub const fn valid(prio: Priority) -> PriorityStatus {
        match prio {
            Priority::Thread(p) => PriorityStatus::Thread(p),
            Priority::Interrupt(p) => PriorityStatus::Interrupt(p),
        }
    }

    pub const fn is_interrupt(&self) -> bool {
        matches!(self, PriorityStatus::Interrupt(_))
    }

    pub const fn is_thread(&self) -> bool {
        matches!(self, PriorityStatus::Thread(_))
    }

    pub const fn is_valid(&self) -> bool {
        !matches!(self, PriorityStatus::Invalid)
    }

    pub const fn priority(&self) -> Option<Priority> {
        match self {
            PriorityStatus::Thread(prio) => Some(Priority::Thread(*prio)),
            PriorityStatus::Interrupt(prio) => Some(Priority::Interrupt(*prio)),
            PriorityStatus::Invalid => None,
        }
    }

    pub const fn get_value(&self) -> u8 {
        match self {
            PriorityStatus::Thread(prio) => *prio,
            PriorityStatus::Interrupt(prio) => *prio,
            PriorityStatus::Invalid => 0,
        }
    }

    pub const fn succ(self) -> PriorityStatus {
        match self {
            PriorityStatus::Thread(prio) => {
                if prio < ThreadPriority::MAX {
                    PriorityStatus::Thread(prio + 1)
                } else {
                    PriorityStatus::valid(Priority::INTERRUPT_MIN)
                }
            }
            PriorityStatus::Interrupt(prio) => {
                if prio < InterruptPriority::MAX {
                    PriorityStatus::Interrupt(prio + 1)
                } else {
                    PriorityStatus::Interrupt(prio)
                }
            }
            PriorityStatus::Invalid => PriorityStatus::Invalid,
        }
    }

    pub const fn max(self, other: PriorityStatus) -> PriorityStatus {
        match (self, other) {
            (PriorityStatus::Thread(p1), PriorityStatus::Thread(p2)) => {
                PriorityStatus::Thread(maxu8(p1, p2))
            }
            (PriorityStatus::Interrupt(p1), PriorityStatus::Interrupt(p2)) => {
                PriorityStatus::Interrupt(maxu8(p1, p2))
            }
            (PriorityStatus::Thread(_), PriorityStatus::Interrupt(_)) => other,
            (PriorityStatus::Interrupt(_), PriorityStatus::Thread(_)) => self,
            (PriorityStatus::Invalid, _) => other,
            (_, PriorityStatus::Invalid) => self,
        }
    }

    pub const fn into_any(self) -> AnyPriority {
        match self {
            PriorityStatus::Thread(prio) => prio as i16,
            PriorityStatus::Interrupt(prio) => prio as i16 | INTERRUPT_BIT,
            PriorityStatus::Invalid => INVALID_PRIORITY,
        }
    }

    pub const fn from_any(value: AnyPriority) -> PriorityStatus {
        if value == INVALID_PRIORITY {
            PriorityStatus::Invalid
        } else if (value & INTERRUPT_BIT) != 0 {
            PriorityStatus::Interrupt((value & !INTERRUPT_BIT) as InterruptPriority)
        } else {
            PriorityStatus::Thread(value as ThreadPriority)
        }
    }
}

impl core::cmp::PartialEq for PriorityStatus {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PriorityStatus::Thread(p1), PriorityStatus::Thread(p2)) => p1 == p2,
            (PriorityStatus::Interrupt(p1), PriorityStatus::Interrupt(p2)) => p1 == p2,
            (PriorityStatus::Invalid, PriorityStatus::Invalid) => true,
            _ => false,
        }
    }
}

impl core::cmp::PartialOrd for PriorityStatus {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        match (self, other) {
            (PriorityStatus::Thread(p1), PriorityStatus::Thread(p2)) => p1.partial_cmp(p2),
            (PriorityStatus::Interrupt(p1), PriorityStatus::Interrupt(p2)) => p1.partial_cmp(p2),
            (PriorityStatus::Thread(_), PriorityStatus::Interrupt(_)) => {
                Some(core::cmp::Ordering::Less)
            }
            (PriorityStatus::Interrupt(_), PriorityStatus::Thread(_)) => {
                Some(core::cmp::Ordering::Greater)
            }
            (PriorityStatus::Invalid, _) => None,
            (_, PriorityStatus::Invalid) => None,
        }
    }
}

impl From<Priority> for PriorityStatus {
    fn from(prio: Priority) -> PriorityStatus {
        match prio {
            Priority::Thread(p) => PriorityStatus::Thread(p),
            Priority::Interrupt(p) => PriorityStatus::Interrupt(p),
        }
    }
}

impl TryFrom<PriorityStatus> for Priority {
    type Error = ();

    fn try_from(prio: PriorityStatus) -> Result<Priority, ()> {
        match prio {
            PriorityStatus::Thread(p) => Ok(Priority::Thread(p)),
            PriorityStatus::Interrupt(p) => Ok(Priority::Interrupt(p)),
            PriorityStatus::Invalid => Err(()),
        }
    }
}

pub struct AtomicPriority(AtomicI16);

impl AtomicPriority {
    pub const fn new(prio: Priority) -> AtomicPriority {
        AtomicPriority::any(prio.into_any())
    }

    pub const fn any(prio: AnyPriority) -> AtomicPriority {
        AtomicPriority(AtomicI16::new(prio))
    }

    pub const fn thread(prio: ThreadPriority) -> AtomicPriority {
        AtomicPriority(AtomicI16::new(prio as i16))
    }

    pub const fn interrupt(prio: InterruptPriority) -> AtomicPriority {
        AtomicPriority(AtomicI16::new(prio as i16 | INTERRUPT_BIT))
    }

    pub fn is_interrupt(&self) -> bool {
        (self.0.load(Ordering::Relaxed) & INTERRUPT_BIT) != 0
    }

    pub fn is_thread(&self) -> bool {
        (self.0.load(Ordering::Relaxed) & INTERRUPT_BIT) == 0
    }

    pub fn load(&self, ordering: Ordering) -> Priority {
        Priority::from_any(self.0.load(ordering))
    }

    pub fn store(&self, prio: Priority, ordering: Ordering) {
        self.0.store(prio.into_any(), ordering)
    }

    pub fn swap(&self, prio: Priority, ordering: Ordering) -> Priority {
        Priority::from_any(self.0.swap(prio.into_any(), ordering))
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
            .map(|x| Priority::from_any(x))
            .map_err(|e| Priority::from_any(e))
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
                f(Priority::from_any(prio)).map(|x| x.into_any())
            })
            .map(|x| Priority::from_any(x))
            .map_err(|e| Priority::from_any(e))
    }
}

const fn from_pair(pair: (Priority, Priority)) -> u32 {
    unsafe { core::mem::transmute::<(Priority, Priority), u32>(pair) }
}

const fn to_pair(value: u32) -> (Priority, Priority) {
    unsafe { core::mem::transmute::<u32, (Priority, Priority)>(value) }
}

const fn from_status_pair(pair: (PriorityStatus, PriorityStatus)) -> u32 {
    pair.0.into_any() as u16 as u32 | ((pair.1.into_any() as u16 as u32) << 16)
}

const fn to_status_pair(value: u32) -> (PriorityStatus, PriorityStatus) {
    (
        PriorityStatus::from_any(value as i16),
        PriorityStatus::from_any((value >> 16) as i16),
    )
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

#[repr(transparent)]
pub struct AtomicPriorityStatusPair(AtomicU32);

impl AtomicPriorityStatusPair {
    pub const fn new(pair: (PriorityStatus, PriorityStatus)) -> AtomicPriorityStatusPair {
        AtomicPriorityStatusPair(AtomicU32::new(from_status_pair(pair)))
    }

    pub fn load(&self, ordering: Ordering) -> (PriorityStatus, PriorityStatus) {
        let value = self.0.load(ordering);
        to_status_pair(value)
    }

    pub fn store(&self, pair: (PriorityStatus, PriorityStatus), ordering: Ordering) {
        let value = from_status_pair(pair);
        self.0.store(value, ordering)
    }

    pub fn swap(
        &self,
        pair: (PriorityStatus, PriorityStatus),
        ordering: Ordering,
    ) -> (PriorityStatus, PriorityStatus) {
        let value = from_status_pair(pair);
        to_status_pair(self.0.swap(value, ordering))
    }

    pub fn compare_exchange(
        &self,
        current: (PriorityStatus, PriorityStatus),
        new: (PriorityStatus, PriorityStatus),
        success: Ordering,
        failure: Ordering,
    ) -> Result<(PriorityStatus, PriorityStatus), (PriorityStatus, PriorityStatus)> {
        self.0
            .compare_exchange(
                from_status_pair(current),
                from_status_pair(new),
                success,
                failure,
            )
            .map(|x| to_status_pair(x))
            .map_err(|e| to_status_pair(e))
    }

    pub fn fetch_update<F>(
        &self,
        set_order: Ordering,
        fetch_order: Ordering,
        mut f: F,
    ) -> Result<(PriorityStatus, PriorityStatus), (PriorityStatus, PriorityStatus)>
    where
        F: FnMut((PriorityStatus, PriorityStatus)) -> Option<(PriorityStatus, PriorityStatus)>,
    {
        self.0
            .fetch_update(set_order, fetch_order, |value| {
                f(to_status_pair(value)).map(|pair| from_status_pair(pair))
            })
            .map(|x| to_status_pair(x))
            .map_err(|e| to_status_pair(e))
    }
}
