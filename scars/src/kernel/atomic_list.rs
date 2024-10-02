use crate::kernel::list::LinkedListTag;
use core::cell::Cell;
use core::marker::PhantomData;
use core::marker::PhantomPinned;
use core::ops::Deref;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

// Intrusive Lock-free FIFO queue
pub(crate) struct AtomicQueue<T, N: LinkedListTag> {
    len: AtomicUsize,
    head: AtomicPtr<AtomicQueueLink<T, N>>,
    tail: AtomicPtr<AtomicQueueLink<T, N>>,
    _phantom: PhantomData<(Cell<AtomicQueueLink<T, N>>, N)>,
}

#[allow(dead_code)]
impl<T: AtomicLinked<N>, N: LinkedListTag> AtomicQueue<T, N> {
    pub const fn new() -> AtomicQueue<T, N> {
        AtomicQueue {
            len: AtomicUsize::new(0),
            head: AtomicPtr::new(core::ptr::null_mut()),
            tail: AtomicPtr::new(core::ptr::null_mut()),
            _phantom: PhantomData,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len.load(Ordering::SeqCst) == 0
    }

    pub fn push_back<'item>(&self, item: &'item T) {
        let inserted_link = item.get_link();
        let inserted_link_ptr = inserted_link.as_ptr();

        // Mark the link as being in a queue
        inserted_link
            .queue
            .compare_exchange(
                core::ptr::null_mut(),
                self as *const _ as *mut _,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .expect("Item is already in a queue");

        // Just a check that next of the inserted item is null
        let old_next = inserted_link.next.load(Ordering::SeqCst);
        assert!(old_next.is_null());

        // Start from tail, which could be a null-pointer
        let mut next = &self.tail;
        let mut tail_ptr = core::ptr::null_mut();
        while let Err(non_null_link_ptr) = next.compare_exchange(
            core::ptr::null_mut(),
            inserted_link_ptr.as_ptr(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            tail_ptr = non_null_link_ptr;
            next = unsafe { &(*non_null_link_ptr).next };
        }

        // Update tail if it matches with the link we just added.
        let _ = self.tail.compare_exchange(
            tail_ptr,
            inserted_link_ptr.as_ptr(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );

        // Tail was changed to inserted link. If tail was null,
        // then head must have been null as well.
        if tail_ptr.is_null() {
            self.head
                .store(inserted_link_ptr.as_ptr(), Ordering::SeqCst);
        }

        self.len.fetch_add(1, Ordering::SeqCst);
    }

    pub fn pop_front<'item>(&self) -> Option<&'item T> {
        loop {
            // Atomically subtract one from queue length. Since `len` is updated at the end
            // of `push`, a `pop` will not see the insertion to empty queue before it is complete.
            if let Err(_) = self
                .len
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |len| {
                    if len > 0 {
                        Some(len - 1)
                    } else {
                        None
                    }
                })
            {
                // Queue is empty
                return None;
            }

            // Read `head` that should never be a null pointer in a queue of non-zero length.
            let head_ptr = self.head.load(Ordering::SeqCst);
            assert!(!head_ptr.is_null());

            // SAFETY: `head_ptr` cannot be null, because it can be null only in
            // empty queue, which was checked above.
            let head_link = unsafe { &*head_ptr };

            // Read pointer of next link. This will become the new head.
            let next = head_link.next.load(Ordering::SeqCst);

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
                head_link
                    .next
                    .store(core::ptr::null_mut(), Ordering::SeqCst);

                // Mark the link as not being in a queue
                head_link
                    .queue
                    .store(core::ptr::null_mut(), Ordering::SeqCst);

                return Some(head_link.get_item());
            }
        }
    }
}

unsafe impl<T, N: LinkedListTag> Send for AtomicQueue<T, N> {}
unsafe impl<T, N: LinkedListTag> Sync for AtomicQueue<T, N> {}

pub(crate) struct AtomicQueueLink<T, N: LinkedListTag> {
    queue: AtomicPtr<AtomicQueue<T, N>>,
    next: AtomicPtr<AtomicQueueLink<T, N>>,
    _pin: PhantomPinned,
    _phantom: PhantomData<(fn(AtomicQueueLink<T, N>) -> AtomicQueueLink<T, N>, N)>,
}

#[allow(dead_code)]
impl<T: AtomicLinked<N>, N: LinkedListTag> AtomicQueueLink<T, N> {
    pub const fn new() -> AtomicQueueLink<T, N> {
        AtomicQueueLink {
            queue: AtomicPtr::new(core::ptr::null_mut()),
            next: AtomicPtr::new(core::ptr::null_mut()),
            _pin: PhantomPinned,
            _phantom: PhantomData,
        }
    }

    fn get_item<'item>(&self) -> &'item T {
        let link_ptr = self as *const AtomicQueueLink<T, N>;
        let link_offset = <T as AtomicLinked<N>>::link_offset();
        let node_ptr = unsafe { (link_ptr as *const u8).sub(link_offset) as *const T };
        unsafe { &*node_ptr }
    }

    fn next_link(&self) -> Option<&AtomicQueueLink<T, N>> {
        unsafe { self.next.load(Ordering::SeqCst).as_ref() }
    }

    fn as_ptr(&self) -> NonNull<AtomicQueueLink<T, N>> {
        unsafe {
            NonNull::new_unchecked(
                self as *const AtomicQueueLink<T, N> as *mut AtomicQueueLink<T, N>,
            )
        }
    }

    pub fn is_linked(&self) -> bool {
        !self.queue.load(Ordering::Relaxed).is_null()
    }
}

#[allow(dead_code)]
pub(crate) trait AtomicLinked<N: LinkedListTag>
where
    Self: Sized,
{
    fn get_link<'item>(&'item self) -> &'item AtomicQueueLink<Self, N>;

    fn get_link_ptr(&self) -> NonNull<AtomicQueueLink<Self, N>> {
        let link = self.get_link();
        NonNull::from(link)
    }

    fn link_offset() -> usize;

    fn get_link_from_ptr<'link>(ptr: NonNull<Self>) -> &'link AtomicQueueLink<Self, N> {
        let node = unsafe { ptr.as_ref() };
        node.get_link()
    }
}

macro_rules! impl_atomic_linked {
    ($link_name:ident, $t:ty, $n:ty) => {
        impl $crate::kernel::atomic_list::AtomicLinked<$n> for $t {
            fn get_link<'item>(
                &'item self,
            ) -> &'item $crate::kernel::atomic_list::AtomicQueueLink<Self, $n> {
                &self.$link_name
            }

            fn link_offset() -> usize {
                ::core::mem::offset_of!($t, $link_name)
            }
        }
    };
}

pub(crate) use impl_atomic_linked;
