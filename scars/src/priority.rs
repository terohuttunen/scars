use crate::sync::atomic::{AtomicI16, Ordering};
use core::marker::ConstParamTy;

pub type ThreadPriority = u8;
pub type InterruptPriority = u8;

pub type AnyPriority = i16;

const INTERRUPT_BIT: i16 = 1 << (i16::BITS - 2);
pub const INVALID_PRIORITY: i16 = -1;

const fn maxu8(left: u8, right: u8) -> u8 {
    if left > right { left } else { right }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ConstParamTy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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

    pub const fn max_valid(self, other: PriorityOpt) -> Priority {
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

/// Like `Priority`, but with an additional `None` variant for the absence of a priority.
#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum PriorityOpt {
    None,
    Thread(ThreadPriority),
    Interrupt(InterruptPriority),
}

impl PriorityOpt {
    pub const fn none() -> PriorityOpt {
        PriorityOpt::None
    }

    pub const fn thread(prio: ThreadPriority) -> PriorityOpt {
        PriorityOpt::Thread(prio)
    }

    pub const fn interrupt(prio: InterruptPriority) -> PriorityOpt {
        PriorityOpt::Interrupt(prio)
    }

    pub const fn some(prio: Priority) -> PriorityOpt {
        match prio {
            Priority::Thread(p) => PriorityOpt::Thread(p),
            Priority::Interrupt(p) => PriorityOpt::Interrupt(p),
        }
    }

    pub const fn is_interrupt(&self) -> bool {
        matches!(self, PriorityOpt::Interrupt(_))
    }

    pub const fn is_thread(&self) -> bool {
        matches!(self, PriorityOpt::Thread(_))
    }

    pub const fn is_some(&self) -> bool {
        !matches!(self, PriorityOpt::None)
    }

    pub const fn is_none(&self) -> bool {
        matches!(self, PriorityOpt::None)
    }

    pub const fn priority(&self) -> Option<Priority> {
        match self {
            PriorityOpt::Thread(prio) => Some(Priority::Thread(*prio)),
            PriorityOpt::Interrupt(prio) => Some(Priority::Interrupt(*prio)),
            PriorityOpt::None => None,
        }
    }

    pub const fn get_value(&self) -> u8 {
        match self {
            PriorityOpt::Thread(prio) => *prio,
            PriorityOpt::Interrupt(prio) => *prio,
            PriorityOpt::None => 0,
        }
    }

    pub const fn succ(self) -> PriorityOpt {
        match self {
            PriorityOpt::Thread(prio) => {
                if prio < ThreadPriority::MAX {
                    PriorityOpt::Thread(prio + 1)
                } else {
                    PriorityOpt::some(Priority::INTERRUPT_MIN)
                }
            }
            PriorityOpt::Interrupt(prio) => {
                if prio < InterruptPriority::MAX {
                    PriorityOpt::Interrupt(prio + 1)
                } else {
                    PriorityOpt::Interrupt(prio)
                }
            }
            PriorityOpt::None => PriorityOpt::None,
        }
    }

    pub const fn max(self, other: PriorityOpt) -> PriorityOpt {
        match (self, other) {
            (PriorityOpt::Thread(p1), PriorityOpt::Thread(p2)) => {
                PriorityOpt::Thread(maxu8(p1, p2))
            }
            (PriorityOpt::Interrupt(p1), PriorityOpt::Interrupt(p2)) => {
                PriorityOpt::Interrupt(maxu8(p1, p2))
            }
            (PriorityOpt::Thread(_), PriorityOpt::Interrupt(_)) => other,
            (PriorityOpt::Interrupt(_), PriorityOpt::Thread(_)) => self,
            (PriorityOpt::None, _) => other,
            (_, PriorityOpt::None) => self,
        }
    }

    pub const fn into_any(self) -> AnyPriority {
        match self {
            PriorityOpt::Thread(prio) => prio as i16,
            PriorityOpt::Interrupt(prio) => prio as i16 | INTERRUPT_BIT,
            PriorityOpt::None => INVALID_PRIORITY,
        }
    }

    pub const fn from_any(value: AnyPriority) -> PriorityOpt {
        if value == INVALID_PRIORITY {
            PriorityOpt::None
        } else if (value & INTERRUPT_BIT) != 0 {
            PriorityOpt::Interrupt((value & !INTERRUPT_BIT) as InterruptPriority)
        } else {
            PriorityOpt::Thread(value as ThreadPriority)
        }
    }

    pub fn unwrap_or_default(self, default: Priority) -> Priority {
        match self {
            PriorityOpt::None => default,
            PriorityOpt::Thread(p) => Priority::Thread(p),
            PriorityOpt::Interrupt(p) => Priority::Interrupt(p),
        }
    }
}

impl core::cmp::PartialEq for PriorityOpt {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PriorityOpt::Thread(p1), PriorityOpt::Thread(p2)) => p1 == p2,
            (PriorityOpt::Interrupt(p1), PriorityOpt::Interrupt(p2)) => p1 == p2,
            (PriorityOpt::None, PriorityOpt::None) => true,
            _ => false,
        }
    }
}

impl core::cmp::PartialOrd for PriorityOpt {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        match (self, other) {
            (PriorityOpt::Thread(p1), PriorityOpt::Thread(p2)) => p1.partial_cmp(p2),
            (PriorityOpt::Interrupt(p1), PriorityOpt::Interrupt(p2)) => p1.partial_cmp(p2),
            (PriorityOpt::Thread(_), PriorityOpt::Interrupt(_)) => Some(core::cmp::Ordering::Less),
            (PriorityOpt::Interrupt(_), PriorityOpt::Thread(_)) => {
                Some(core::cmp::Ordering::Greater)
            }
            (PriorityOpt::None, _) => None,
            (_, PriorityOpt::None) => None,
        }
    }
}

impl From<Priority> for PriorityOpt {
    fn from(prio: Priority) -> PriorityOpt {
        match prio {
            Priority::Thread(p) => PriorityOpt::Thread(p),
            Priority::Interrupt(p) => PriorityOpt::Interrupt(p),
        }
    }
}

impl TryFrom<PriorityOpt> for Priority {
    type Error = ();

    fn try_from(prio: PriorityOpt) -> Result<Priority, ()> {
        match prio {
            PriorityOpt::Thread(p) => Ok(Priority::Thread(p)),
            PriorityOpt::Interrupt(p) => Ok(Priority::Interrupt(p)),
            PriorityOpt::None => Err(()),
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

#[repr(transparent)]
pub struct AtomicPriorityOpt(AtomicI16);

impl AtomicPriorityOpt {
    pub const fn new(prio: PriorityOpt) -> AtomicPriorityOpt {
        AtomicPriorityOpt(AtomicI16::new(prio.into_any()))
    }

    pub const fn any(prio: AnyPriority) -> AtomicPriorityOpt {
        AtomicPriorityOpt(AtomicI16::new(prio))
    }

    pub const fn thread(prio: ThreadPriority) -> AtomicPriorityOpt {
        AtomicPriorityOpt(AtomicI16::new(prio as i16))
    }

    pub const fn interrupt(prio: InterruptPriority) -> AtomicPriorityOpt {
        AtomicPriorityOpt(AtomicI16::new(prio as i16 | INTERRUPT_BIT))
    }

    pub fn is_interrupt(&self) -> bool {
        (self.0.load(Ordering::Relaxed) & INTERRUPT_BIT) != 0
    }

    pub fn is_thread(&self) -> bool {
        (self.0.load(Ordering::Relaxed) & INTERRUPT_BIT) == 0
    }

    pub fn load(&self, ordering: Ordering) -> PriorityOpt {
        PriorityOpt::from_any(self.0.load(ordering))
    }

    pub fn store(&self, prio: PriorityOpt, ordering: Ordering) {
        self.0.store(prio.into_any(), ordering)
    }

    pub fn swap(&self, prio: PriorityOpt, ordering: Ordering) -> PriorityOpt {
        PriorityOpt::from_any(self.0.swap(prio.into_any(), ordering))
    }

    pub fn compare_exchange(
        &self,
        current: PriorityOpt,
        new: PriorityOpt,
        success: Ordering,
        failure: Ordering,
    ) -> Result<PriorityOpt, PriorityOpt> {
        self.0
            .compare_exchange(current.into_any(), new.into_any(), success, failure)
            .map(|x| PriorityOpt::from_any(x))
            .map_err(|e| PriorityOpt::from_any(e))
    }

    pub fn fetch_update<F>(
        &self,
        set_order: Ordering,
        fetch_order: Ordering,
        mut f: F,
    ) -> Result<PriorityOpt, PriorityOpt>
    where
        F: FnMut(PriorityOpt) -> Option<PriorityOpt>,
    {
        self.0
            .fetch_update(set_order, fetch_order, |prio| {
                f(PriorityOpt::from_any(prio)).map(|x| x.into_any())
            })
            .map(|x| PriorityOpt::from_any(x))
            .map_err(|e| PriorityOpt::from_any(e))
    }
}
