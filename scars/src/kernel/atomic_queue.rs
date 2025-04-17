use crate::kernel::list::LinkedListTag;
use core::cell::Cell;
use core::marker::PhantomData;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use unrecoverable_error::{UnrecoverableError, unrecoverable_error};

#[derive(Debug, UnrecoverableError)]
pub enum AtomicQueueError {
    ItemAlreadyInQueue,
    ItemNotInQueue,
}

/// Intrusive Lock-free FIFO queue
///
/// Implementation notes:
///  - The sequence of atomic operations is based on the Michael-Scott algorithm.
pub(crate) struct AtomicQueue<T, N: LinkedListTag> {
    sentinel: AtomicNode<T, N>,
    head: AtomicPtr<AtomicNode<T, N>>,
    tail: AtomicPtr<AtomicNode<T, N>>,
    _phantom: PhantomData<(*const AtomicNode<T, N>, N)>,
    _pin: PhantomPinned,
}

#[allow(dead_code)]
impl<T, N: LinkedListTag> AtomicQueue<T, N> {
    /// Creates a new, empty AtomicQueue in uninitialized state.
    pub const fn new() -> AtomicQueue<T, N> {
        AtomicQueue {
            sentinel: AtomicNode::new(),
            head: AtomicPtr::new(core::ptr::null_mut()),
            tail: AtomicPtr::new(core::ptr::null_mut()),
            _phantom: PhantomData,
            _pin: PhantomPinned,
        }
    }

    /// Initializes the queue if it's not already initialized
    fn init_once(&'static self) {
        let sentinel_ptr: *mut AtomicNode<T, N> = &self.sentinel as *const _ as *mut _;
        // In empty queue, head and tail point to the sentinel.
        // Head and tail are null only if the queue is not initialized.
        let _ = self.head.compare_exchange(
            core::ptr::null_mut(),
            sentinel_ptr,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        let _ = self.tail.compare_exchange(
            core::ptr::null_mut(),
            sentinel_ptr,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    // Checks if the queue is empty.
    // It's empty if head's next pointer is null.
    pub fn is_empty(&'static self) -> bool {
        self.init_once();

        let head_ptr = self.head.load(Ordering::Acquire);
        // SAFETY: head always points to a valid sentinel node.
        let head_node = unsafe { &*head_ptr };
        // The queue is empty if the node after head (the sentinel) is null.
        head_node.next.load(Ordering::Acquire).is_null()
    }

    // Enqueues an item at the tail of the queue.
    pub fn push_back<'item>(&'static self, item: Pin<&'item T>)
    where
        T: AtomicQueueNode<N>,
    {
        self.init_once();

        let new_node = item.get_node();
        let new_node_ptr = new_node.as_ptr().as_ptr();

        if new_node
            .owned
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            unrecoverable_error!(AtomicQueueError::ItemAlreadyInQueue);
        }

        // Ensure next is null initially
        new_node
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);

        loop {
            let tail_ptr = self.tail.load(Ordering::Acquire);
            // SAFETY: tail always points to a valid node (sentinel or actual item).
            let tail_node = unsafe { &*tail_ptr };
            let next_ptr = tail_node.next.load(Ordering::Acquire);

            // Is tail pointer up-to-date?
            // Check if the tail we loaded still matches the current tail.
            if tail_ptr != self.tail.load(Ordering::Acquire) {
                continue; // Tail has been updated by another thread, retry.
            }

            if next_ptr.is_null() {
                // Tail is pointing to the last node. Try to link the new node.
                if tail_node
                    .next
                    .compare_exchange(
                        core::ptr::null_mut(),
                        new_node_ptr,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    // Successfully linked the new node. Try to advance the tail pointer.
                    // It's ok if this fails, another thread might have already done it.
                    let _ = self.tail.compare_exchange(
                        tail_ptr,
                        new_node_ptr,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    );
                    return; // Enqueue successful
                }
            } else {
                // Tail pointer is lagging behind the actual last node.
                // Try to advance the tail pointer to the next node.
                // It's ok if this fails, another thread might have already done it.
                let _ = self.tail.compare_exchange(
                    tail_ptr,
                    next_ptr,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                );
                // Retry the loop regardless of CAS success/failure
            }
        }
    }

    // Dequeues an item from the head of the queue.
    pub fn pop_front<'item>(&'static self) -> Option<Pin<&'item T>>
    where
        T: AtomicQueueNode<N>,
    {
        self.init_once();

        loop {
            let head_ptr = self.head.load(Ordering::Acquire);
            // SAFETY: head always points to a valid node (sentinel or actual item).
            let head_node = unsafe { &*head_ptr };
            let tail_ptr = self.tail.load(Ordering::Acquire);
            let next_ptr = head_node.next.load(Ordering::Acquire);

            // Is head pointer up-to-date?
            // Check if the head we loaded still matches the current head.
            if head_ptr != self.head.load(Ordering::Acquire) {
                continue; // Head changed, retry
            }

            if head_ptr == tail_ptr {
                // Head and tail pointing to the same node.
                if next_ptr.is_null() {
                    // Queue is empty (or potentially tail is lagging, but next being null confirms empty after sentinel).
                    return None;
                } else {
                    // Tail is lagging behind head. Try to advance tail.
                    let _ = self.tail.compare_exchange(
                        tail_ptr,
                        next_ptr,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    );
                    // Retry the loop after attempting to advance tail.
                    continue;
                }
            } else {
                // Head and tail are different. Read value before CAS.
                // Ensure next_ptr is not null, otherwise it's an invalid state (should have been caught by head == tail check).
                if next_ptr.is_null() {
                    // This case should logically not happen if head != tail in a correct MS queue.
                    // Could indicate memory corruption or race condition logic error elsewhere.
                    // Spin or panic depending on safety requirements. For now, retry.
                    continue;
                }

                // Try to move head to the next node.
                if self
                    .head
                    .compare_exchange(head_ptr, next_ptr, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    // Successfully moved head. head_ptr now points to the dequeued node (which was the *original* head's next).
                    // SAFETY: next_ptr was confirmed non-null and points to a valid node structure
                    // because it came from a loaded AtomicPtr that was successfully CAS'd.
                    // The node it points to contains a valid T because it was previously enqueued.
                    let dequeued_node = unsafe { &*next_ptr };
                    if dequeued_node
                        .owned
                        .compare_exchange(true, false, Ordering::SeqCst, Ordering::Relaxed)
                        .is_err()
                    {
                        unrecoverable_error!(AtomicQueueError::ItemNotInQueue);
                    }
                    return Some(dequeued_node.get_item()); // Return the item from the *new* head.
                }
                // CAS failed, head was modified by another thread. Retry the loop.
            }
        }
    }
}

unsafe impl<T, N: LinkedListTag> Send for AtomicQueue<T, N> {}
unsafe impl<T, N: LinkedListTag> Sync for AtomicQueue<T, N> {}

#[derive(Debug)]
pub(crate) struct AtomicNode<T, N: LinkedListTag> {
    owned: AtomicBool,
    next: AtomicPtr<AtomicNode<T, N>>,
    _pin: PhantomPinned,
    _phantom: PhantomData<(fn(AtomicNode<T, N>) -> AtomicNode<T, N>, N)>,
}

#[allow(dead_code)]
impl<T, N: LinkedListTag> AtomicNode<T, N> {
    // Creates a new node, typically used for the sentinel.
    pub const fn new() -> AtomicNode<T, N> {
        AtomicNode {
            owned: AtomicBool::new(false),
            next: AtomicPtr::new(core::ptr::null_mut()),
            _pin: PhantomPinned,
            _phantom: PhantomData,
        }
    }

    // Retrieves the item associated with this node.
    // Requires T: AtomicQueueNode<N> context.
    fn get_item<'item>(&self) -> Pin<&'item T>
    where
        T: AtomicQueueNode<N>,
    {
        let link_ptr = self as *const AtomicNode<T, N>;
        let link_offset = <T as AtomicQueueNode<N>>::node_offset();
        let node_ptr = unsafe { (link_ptr as *const u8).sub(link_offset) as *const T };
        unsafe { Pin::new_unchecked(&*node_ptr) }
    }

    // Returns the next node in the queue if it exists.
    fn next_node(&self) -> Option<&AtomicNode<T, N>> {
        unsafe { self.next.load(Ordering::Acquire).as_ref() }
    }

    // Gets a NonNull pointer to this node.
    fn as_ptr(&self) -> NonNull<AtomicNode<T, N>> {
        unsafe { NonNull::new_unchecked(self as *const AtomicNode<T, N> as *mut AtomicNode<T, N>) }
    }

    pub fn is_linked(&self) -> bool {
        self.owned.load(Ordering::SeqCst)
    }
}

#[allow(dead_code)]
pub(crate) trait AtomicQueueNode<N: LinkedListTag>
where
    Self: Sized,
{
    fn get_node(&self) -> &AtomicNode<Self, N>;

    fn get_node_ptr(&self) -> NonNull<AtomicNode<Self, N>> {
        let link = self.get_node();
        NonNull::from(link)
    }

    fn node_offset() -> usize;

    fn get_node_from_ptr<'link>(ptr: NonNull<Self>) -> &'link AtomicNode<Self, N> {
        let node = unsafe { ptr.as_ref() };
        node.get_node()
    }
}

macro_rules! impl_atomic_linked {
    ($node_name:ident, $t:ty, $n:ty) => {
        impl $crate::kernel::atomic_queue::AtomicQueueNode<$n> for $t {
            fn get_node(&self) -> &$crate::kernel::atomic_queue::AtomicNode<Self, $n> {
                &self.$node_name
            }

            fn node_offset() -> usize {
                ::core::mem::offset_of!($t, $node_name)
            }
        }
    };
}

pub(crate) use impl_atomic_linked;

#[cfg(test)]
mod tests {
    use super::*;
    use core::pin::Pin;

    // 1. Define a Tag for the test list
    #[derive(Debug)]
    struct TestTag;
    impl LinkedListTag for TestTag {}

    // 2. Define the data structure containing the node
    #[derive(Debug)]
    struct TestData {
        node: AtomicNode<TestData, TestTag>,
        value: u32,
    }

    // 3. Implement the trait using the macro
    // Ensure the path in the macro matches the module location if moved
    impl_atomic_linked!(node, TestData, TestTag);

    impl TestData {
        // Const constructor for static initialization
        const fn new(value: u32) -> Self {
            TestData {
                value,
                node: AtomicNode::new(), // Initialize the node
            }
        }
    }

    // 4. Create static nodes for sentinel and test items
    // SAFETY: These are static and their address is stable. Pinning is sound.
    static SENTINEL_NODE: AtomicNode<TestData, TestTag> = AtomicNode::new();
    static ITEM1: TestData = TestData::new(10);
    static ITEM2: TestData = TestData::new(20);
    static ITEM3: TestData = TestData::new(30);

    #[test_case]
    fn test_new_empty() {
        static QUEUE: AtomicQueue<TestData, TestTag> = AtomicQueue::new();
        assert!(QUEUE.is_empty());
    }

    #[test_case]
    fn test_pop_empty() {
        static QUEUE: AtomicQueue<TestData, TestTag> = AtomicQueue::new();
        assert!(QUEUE.pop_front().is_none());
        assert!(QUEUE.is_empty());
    }

    #[test_case]
    fn test_push_pop_single() {
        // Ensure nodes are reset for the test if tests run concurrently (statics!)
        // Basic tests usually run sequentially, but good practice to consider.
        // For simplicity here, we assume sequential execution or separate static instances per test if needed.
        // Resetting static node state:
        SENTINEL_NODE
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);
        ITEM1
            .node
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);

        static QUEUE: AtomicQueue<TestData, TestTag> = AtomicQueue::new();

        let item1_pin = Pin::static_ref(&ITEM1);

        assert!(QUEUE.is_empty());
        QUEUE.push_back(item1_pin);
        assert!(!QUEUE.is_empty());

        let popped_item = QUEUE.pop_front();
        assert!(popped_item.is_some());

        // Compare by pointer address for static items, or by value
        let popped_ref = popped_item.unwrap();
        assert_eq!(popped_ref.value, ITEM1.value);
        // Check if the pointers point to the same static item
        assert!(core::ptr::eq(popped_ref.get_ref(), &ITEM1));

        assert!(QUEUE.is_empty());
        assert!(QUEUE.pop_front().is_none()); // Ensure queue is empty after pop
    }

    #[test_case]
    fn test_push_pop_multiple() {
        // Reset static nodes
        SENTINEL_NODE
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);
        ITEM1
            .node
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);
        ITEM2
            .node
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);
        ITEM3
            .node
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);

        static QUEUE: AtomicQueue<TestData, TestTag> = AtomicQueue::new();

        let item1_pin = Pin::static_ref(&ITEM1);
        let item2_pin = Pin::static_ref(&ITEM2);
        let item3_pin = Pin::static_ref(&ITEM3);

        assert!(QUEUE.is_empty());

        QUEUE.push_back(item1_pin);
        assert!(!QUEUE.is_empty());
        QUEUE.push_back(item2_pin);
        assert!(!QUEUE.is_empty());
        QUEUE.push_back(item3_pin);
        assert!(!QUEUE.is_empty());

        // Pop and check FIFO order
        let popped1 = QUEUE.pop_front();
        assert!(popped1.is_some());
        assert_eq!(popped1.unwrap().value, ITEM1.value);
        assert!(!QUEUE.is_empty());

        let popped2 = QUEUE.pop_front();
        assert!(popped2.is_some());
        assert_eq!(popped2.unwrap().value, ITEM2.value);
        assert!(!QUEUE.is_empty());

        let popped3 = QUEUE.pop_front();
        assert!(popped3.is_some());
        assert_eq!(popped3.unwrap().value, ITEM3.value);
        assert!(QUEUE.is_empty());

        // Check queue is empty after all pops
        assert!(QUEUE.pop_front().is_none());
        assert!(QUEUE.is_empty());
    }
}
