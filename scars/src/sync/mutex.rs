use crate::kernel::Priority;
use crate::kernel::hal::CoreId;
use crate::sync::guarded::{Guard, Guarded};
use crate::sync::{CoreCeilingLock, CoreInheritanceLock, CoreInterruptLock};

pub type Mutex<T, const CORE: CoreId = { CoreId::DEFAULT }> = Guarded<T, CoreInheritanceLock<CORE>>;
pub type CeilingMutex<T, const CEILING: Priority, const CORE: CoreId = { CoreId::DEFAULT }> =
    Guarded<T, CoreCeilingLock<CEILING, CORE>>;
pub type InterruptMutex<T, const CORE: CoreId = { CoreId::DEFAULT }> =
    Guarded<T, CoreInterruptLock<CORE>>;

pub type MutexGuard<'a, T, L> = Guard<'a, T, L>;
