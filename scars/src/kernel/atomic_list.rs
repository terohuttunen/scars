use crate::kernel::list::LinkedListTag;
use core::cell::Cell;
use core::marker::PhantomData;
use core::marker::PhantomPinned;
use core::ops::Deref;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

// Intrusive Lock-free FIFO queue
pub(crate) struct AtomicQueue<T, N: LinkedListTag> {
    head: AtomicPtr<AtomicNode<T, N>>,
    tail: AtomicPtr<AtomicNode<T, N>>,
    _phantom: PhantomData<(Cell<AtomicNode<T, N>>, N)>,
}

#[allow(dead_code)]
impl<T: AtomicQueueNode<N>, N: LinkedListTag> AtomicQueue<T, N> {
    pub const fn new() -> AtomicQueue<T, N> {
        AtomicQueue {
            head: AtomicPtr::new(core::ptr::null_mut()),
            tail: AtomicPtr::new(core::ptr::null_mut()),
            _phantom: PhantomData,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::SeqCst).is_null()
    }

    pub fn push_back<'item>(&self, item: Pin<&'item T>) {
        let inserted_node = item.get_node();
        let inserted_node_ptr = inserted_node.as_ptr();

        // Mark the link as being in a queue
        inserted_node
            .owned
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .expect("Item is already in a queue");

        // Just a check that next of the inserted item is null
        let old_next = inserted_node.next.load(Ordering::SeqCst);
        assert!(old_next.is_null());

        // Start from tail, which could be a null-pointer
        let mut next = &self.tail;
        let mut tail_ptr = core::ptr::null_mut();
        while let Err(non_null_link_ptr) = next.compare_exchange(
            core::ptr::null_mut(),
            inserted_node_ptr.as_ptr(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            tail_ptr = non_null_link_ptr;
            next = unsafe { &(*non_null_link_ptr).next };
        }

        // Update tail if it matches with the link we just added.
        let _ = self.tail.compare_exchange(
            tail_ptr,
            inserted_node_ptr.as_ptr(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );

        // Tail was changed to inserted link. If tail was null,
        // then head must have been null as well.
        if tail_ptr.is_null() {
            self.head
                .store(inserted_node_ptr.as_ptr(), Ordering::SeqCst);
        }
    }

    pub fn pop_front<'item>(&self) -> Option<Pin<&'item T>> {
        loop {
            // Read `head` that should never be a null pointer in a queue of non-zero length.
            let head_ptr = self.head.load(Ordering::SeqCst);
            if head_ptr.is_null() {
                // Queue is empty
                return None;
            }

            // SAFETY: `head_ptr` cannot be null, because it can be null only in
            // empty queue, which was checked above.
            let head_node = unsafe { &*head_ptr };

            // Read pointer of next link. This will become the new head.
            let next = head_node.next.load(Ordering::SeqCst);

            // Try to exchange head with next
            if let Ok(_) =
                self.head
                    .compare_exchange(head_ptr, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                // If `next` is null, then queue had only one item when it was read.
                // When queue has only one item, `head` and `tail` point to same link,
                // and `tail` must be updated as well when `head` changes.
                if next.is_null() {
                    // If `tail` is still same as `head`, no new items have been added to the
                    // queue since the one was removed from it, and it can be set to null.
                    if let Err(ptr) = self.tail.compare_exchange(
                        head_ptr,
                        core::ptr::null_mut(),
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        // `tail` was no longer the same as `head`, so new elements have
                        // been added. Update head to point to those new elements.
                        self.head.store(ptr, Ordering::SeqCst);
                    }
                }
                // The old head-link still points to the same next link and must be detached.
                head_node
                    .next
                    .store(core::ptr::null_mut(), Ordering::SeqCst);

                // Mark the link as not being in a queue
                head_node.owned.store(false, Ordering::SeqCst);

                return Some(head_node.get_item());
            }
        }
    }
}

unsafe impl<T, N: LinkedListTag> Send for AtomicQueue<T, N> {}
unsafe impl<T, N: LinkedListTag> Sync for AtomicQueue<T, N> {}

pub(crate) struct AtomicNode<T, N: LinkedListTag> {
    owned: AtomicBool,
    next: AtomicPtr<AtomicNode<T, N>>,
    _pin: PhantomPinned,
    _phantom: PhantomData<(fn(AtomicNode<T, N>) -> AtomicNode<T, N>, N)>,
}

#[allow(dead_code)]
impl<T: AtomicQueueNode<N>, N: LinkedListTag> AtomicNode<T, N> {
    pub const fn new() -> AtomicNode<T, N> {
        AtomicNode {
            owned: AtomicBool::new(false),
            next: AtomicPtr::new(core::ptr::null_mut()),
            _pin: PhantomPinned,
            _phantom: PhantomData,
        }
    }

    fn get_item<'item>(&self) -> Pin<&'item T> {
        let link_ptr = self as *const AtomicNode<T, N>;
        let link_offset = <T as AtomicQueueNode<N>>::node_offset();
        let node_ptr = unsafe { (link_ptr as *const u8).sub(link_offset) as *const T };
        unsafe { Pin::new_unchecked(&*node_ptr) }
    }

    fn next_node(&self) -> Option<&AtomicNode<T, N>> {
        unsafe { self.next.load(Ordering::SeqCst).as_ref() }
    }

    fn as_ptr(&self) -> NonNull<AtomicNode<T, N>> {
        unsafe { NonNull::new_unchecked(self as *const AtomicNode<T, N> as *mut AtomicNode<T, N>) }
    }

    pub fn is_linked(&self) -> bool {
        self.owned.load(Ordering::Relaxed)
    }
}

#[allow(dead_code)]
pub(crate) trait AtomicQueueNode<N: LinkedListTag>
where
    Self: Sized,
{
    fn get_node<'item>(&'item self) -> &'item AtomicNode<Self, N>;

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
        impl $crate::kernel::atomic_list::AtomicQueueNode<$n> for $t {
            fn get_node<'item>(
                &'item self,
            ) -> &'item $crate::kernel::atomic_list::AtomicNode<Self, $n> {
                &self.$node_name
            }

            fn node_offset() -> usize {
                ::core::mem::offset_of!($t, $node_name)
            }
        }
    };
}

pub(crate) use impl_atomic_linked;
