//! Asynchronous resource lock that has the semantics of a Rust Mutex. Nested per-resource locks
//! are not allowed. Allows multiple tasks to access and modify a resource concurrently, but
//! only one task at a time.
//!
use crate::kernel::list::LinkedList;
use crate::kernel::waiter::{WaitQueue, WaitQueueTag};
use crate::task::TaskControlBlock;

pub struct AsyncLock {
    owner: Option<*const TaskControlBlock>,
    queue: LinkedList<TaskControlBlock, WaitQueueTag>,
}
