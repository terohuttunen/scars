use crate::kernel::list::LinkedListTag;
use crate::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use core::cell::Cell;
use core::marker::PhantomData;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::ptr::NonNull;
use scars_fault::{Fault, fault};

#[derive(Fault)]
pub enum AtomicQueueError {
    ItemAlreadyInQueue,
}

/// Intrusive lock-free MPSC FIFO queue (Vyukov's algorithm).
///
/// Multiple producers may push concurrently. At most one consumer may
/// pop at a time.
///
/// A node that has been popped may be pushed again immediately. `push`
/// resets the new node's `next` to `null`, atomically swaps it in as
/// the producer-end head, then writes `prev.next = new`; the linkage
/// store always targets a different node from `new`.
///
/// `pop` returns `None` between a producer's `XCHG` and its linkage
/// store. Producers must follow every push with a wake-up notification
/// to the consumer (e.g. `pend_service_call`); the consumer then runs
/// again once the linkage is visible.
pub(crate) struct AtomicQueue<T, N: LinkedListTag> {
    /// Permanent sentinel; remains in the chain across all states.
    sentinel: AtomicNode<T, N>,
    /// Producer-end pointer. `swap`-updated on push; the prior value
    /// is the predecessor whose `next` is then linked to the new node.
    head: AtomicPtr<AtomicNode<T, N>>,
    /// Consumer cursor. Read and written only by the single consumer.
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

    /// Initializes the queue if it's not already initialized. Both
    /// head and tail start out pointing at the sentinel.
    fn init_once(&'static self) {
        let sentinel_ptr: *mut AtomicNode<T, N> = &self.sentinel as *const _ as *mut _;
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

    /// True when no data nodes are queued.
    pub fn is_empty(&'static self) -> bool {
        self.init_once();
        let sentinel_ptr: *mut AtomicNode<T, N> = &self.sentinel as *const _ as *mut _;
        // Head equals sentinel iff no producer has installed a node
        // since the last full drain.
        self.head.load(Ordering::Acquire) == sentinel_ptr
    }

    pub fn push_back<'item>(&'static self, item: Pin<&'item T>)
    where
        T: AtomicQueueNode<N>,
    {
        if let Err(e) = self.try_push_back(item) {
            fault!(e);
        }
    }

    /// Atomically enqueue `item`. Returns `ItemAlreadyInQueue` if the
    /// node is currently linked.
    pub fn try_push_back<'item>(&'static self, item: Pin<&'item T>) -> Result<(), AtomicQueueError>
    where
        T: AtomicQueueNode<N>,
    {
        self.init_once();

        let new_node = item.get_node();
        let new_node_ptr = new_node.as_ptr().as_ptr();

        if new_node
            .owned
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(AtomicQueueError::ItemAlreadyInQueue);
        }

        new_node
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);

        let prev = self.head.swap(new_node_ptr, Ordering::AcqRel);
        // SAFETY: `prev` is either the sentinel or a previously-pushed
        // node still anchored in the chain. The release store
        // synchronises with the consumer's acquire load of `next`.
        unsafe {
            (*prev).next.store(new_node_ptr, Ordering::Release);
        }
        Ok(())
    }

    /// Dequeues the front item.
    ///
    /// Single-consumer: at most one caller at a time. May return
    /// `None` while a producer is between its `swap` and its linkage
    /// store, even if items are queued.
    pub fn pop_front<'item>(&'static self) -> Option<Pin<&'item T>>
    where
        T: AtomicQueueNode<N>,
    {
        self.init_once();
        let sentinel_ptr: *mut AtomicNode<T, N> = &self.sentinel as *const _ as *mut _;

        let mut tail = self.tail.load(Ordering::Relaxed);
        let mut next = unsafe { (*tail).next.load(Ordering::Acquire) };

        if tail == sentinel_ptr {
            if next.is_null() {
                return None;
            }
            self.tail.store(next, Ordering::Relaxed);
            tail = next;
            next = unsafe { (*tail).next.load(Ordering::Acquire) };
        }

        if !next.is_null() {
            self.tail.store(next, Ordering::Relaxed);
            // SAFETY: `tail` is a data node — the sentinel-skip above
            // guarantees it.
            unsafe { (*tail).owned.store(false, Ordering::Release) };
            return Some(unsafe { (*tail).get_item() });
        }

        let head = self.head.load(Ordering::Acquire);
        if tail != head {
            // Producer mid-push: head was swapped, linkage not yet
            // visible.
            return None;
        }

        // Single data node remains and it equals head; re-inject the
        // sentinel so `tail.next` becomes non-null and the advance below
        // succeeds.
        self.sentinel
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);
        let prev = self.head.swap(sentinel_ptr, Ordering::AcqRel);
        unsafe {
            (*prev).next.store(sentinel_ptr, Ordering::Release);
        }

        let next2 = unsafe { (*tail).next.load(Ordering::Acquire) };
        if !next2.is_null() {
            self.tail.store(next2, Ordering::Relaxed);
            unsafe { (*tail).owned.store(false, Ordering::Release) };
            return Some(unsafe { (*tail).get_item() });
        }
        None
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

    /// A node that was just popped must be pushable again immediately.
    #[test_case]
    fn test_repush_after_pop() {
        ITEM1
            .node
            .next
            .store(core::ptr::null_mut(), Ordering::Relaxed);
        ITEM1.node.owned.store(false, Ordering::Relaxed);

        static QUEUE: AtomicQueue<TestData, TestTag> = AtomicQueue::new();
        let item1_pin = Pin::static_ref(&ITEM1);

        for _ in 0..3 {
            QUEUE.push_back(item1_pin);
            assert!(!QUEUE.is_empty());
            let popped = QUEUE.pop_front().expect("must pop the item we just pushed");
            assert!(core::ptr::eq(popped.get_ref(), &ITEM1));
            assert!(QUEUE.is_empty());
            assert!(QUEUE.pop_front().is_none());
        }
    }
}
