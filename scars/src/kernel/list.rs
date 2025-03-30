use crate::cell::LockedRefCell;
use core::cell::{Cell, Ref, RefMut};
use core::marker::PhantomData;
use core::marker::PhantomPinned;
use core::ops::Deref;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

pub trait LinkedListTag: 'static {}

pub(crate) struct LinkedList<T: LinkedListNode<N>, N: LinkedListTag> {
    pub(crate) head: Option<NonNull<Node<T, N>>>,
    pub(crate) tail: Option<NonNull<Node<T, N>>>,
    _phantom: PhantomData<(Cell<Node<T, N>>, N)>,
    _pin: PhantomPinned,
}

#[allow(dead_code)]
impl<T: LinkedListNode<N>, N: LinkedListTag> LinkedList<T, N> {
    pub const fn new() -> LinkedList<T, N> {
        LinkedList {
            head: None,
            tail: None,
            _phantom: PhantomData,
            _pin: PhantomPinned,
        }
    }

    pub fn head<'item>(self: Pin<&Self>) -> Option<Pin<&'item T>> {
        self.head
            .map(|node_ptr| unsafe { node_ptr.as_ref().get_item() })
    }

    pub fn tail<'item>(self: Pin<&Self>) -> Option<Pin<&'item T>> {
        self.tail
            .map(|node_ptr| unsafe { node_ptr.as_ref().get_item() })
    }

    fn replace_head(
        self: Pin<&mut Self>,
        head: Option<NonNull<Node<T, N>>>,
    ) -> Option<NonNull<Node<T, N>>> {
        let list = unsafe { self.get_unchecked_mut() };
        core::mem::replace(&mut list.head, head)
    }

    fn replace_tail(
        self: Pin<&mut Self>,
        tail: Option<NonNull<Node<T, N>>>,
    ) -> Option<NonNull<Node<T, N>>> {
        let list = unsafe { self.get_unchecked_mut() };
        core::mem::replace(&mut list.tail, tail)
    }

    pub fn push_front<'item>(mut self: Pin<&mut Self>, item: Pin<&'item T>) {
        let node = item.get_node();
        let node_ptr = item.get_node_ptr();

        // Take ownership of the node
        node.owned
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .expect("Item pushed into a list cannot be a member of a list");

        // Adjust old head links
        if let Some(head) = self.as_ref().head() {
            let head_link = head.get_node();
            head_link.prev.set(Some(node_ptr));
        }

        // Adjust new node links
        node.next.set(self.head);
        node.prev.set(None);

        // Replace head with new node
        self.as_mut().replace_head(Some(node_ptr));
        if self.tail.is_none() {
            // Replace also tail if list was empty
            self.replace_tail(Some(node_ptr));
        }
    }

    pub fn push_back<'item>(mut self: Pin<&mut Self>, item: Pin<&'item T>) {
        let node = item.get_node();
        let node_ptr = item.get_node_ptr();

        // Take ownership of the node
        node.owned
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .expect("Item pushed into a list cannot be a member of a list");

        // Adjust old tail links
        if let Some(tail) = self.as_ref().tail() {
            let tail_node = tail.get_node();
            tail_node.next.set(Some(node_ptr));
        }

        // Adjust new node links
        node.next.set(None);
        node.prev.set(self.tail);

        // Replace tail with new node
        self.as_mut().replace_tail(Some(node_ptr));
        if self.head.is_none() {
            // Replace also head if list was empty
            self.replace_head(Some(node_ptr));
        }
    }

    pub fn pop_front<'item>(mut self: Pin<&mut Self>) -> Option<Pin<&'item T>> {
        if let Some(head) = self
            .as_mut()
            .replace_head(None)
            .map(|head| unsafe { head.as_ref().get_item() })
        {
            let head_node = head.get_node();
            if let Some(next_node) = head_node.next_node() {
                next_node.prev.set(None);
                self.as_mut().replace_head(Some(next_node.as_ptr()));
            } else {
                self.replace_tail(None);
            }
            head_node.next.set(None);
            head_node.prev.set(None);
            head_node.owned.store(false, Ordering::Relaxed);
            return Some(head);
        }

        None
    }

    pub fn pop_front_if<'item, P: FnOnce(&T) -> bool>(
        self: Pin<&mut Self>,
        predicate: P,
    ) -> Option<Pin<&'item T>> {
        if let Some(head) = self.as_ref().head() {
            if predicate(&*head) {
                return self.pop_front();
            }
        }

        None
    }

    pub fn cursor_front(self: Pin<&Self>) -> Cursor<'_, T, N> {
        Cursor {
            current: self.head,
            list: self,
        }
    }

    pub fn cursor_front_mut(self: Pin<&mut Self>) -> CursorMut<'_, T, N> {
        CursorMut {
            current: self.head,
            list: self,
        }
    }

    // Insert to list after predicate becomes false
    pub fn insert_after<'item, P>(self: Pin<&mut Self>, item: Pin<&'item T>, predicate: P)
    where
        P: Fn(&T) -> bool,
    {
        let mut cursor = self.cursor_front_mut();
        loop {
            match cursor.get_item() {
                Some(list_item) if predicate(&*list_item) => {
                    cursor.move_next();
                }
                _ => {
                    cursor.insert_before(item);
                    break;
                }
            }
        }
    }

    /// Safety: The caller must guarantee that the item is in the list.
    pub unsafe fn remove<'item>(mut self: Pin<&mut Self>, item: Pin<&'item T>) {
        let node = item.get_node();
        node.owned
            .compare_exchange(true, false, Ordering::Acquire, Ordering::Relaxed)
            .expect("Cannot remove item that is not in a list");

        if let Some(prev_node) = node.prev_node() {
            prev_node.next.set(node.next.get());
        } else {
            self.as_mut().replace_head(node.next.get());
        }

        if let Some(next_link) = node.next_node() {
            next_link.prev.set(node.prev.get());
        } else {
            self.replace_tail(node.prev.get());
        }

        node.next.set(None);
        node.prev.set(None);
    }
}

pub(crate) struct Node<T: LinkedListNode<N>, N: LinkedListTag> {
    /// True if the node is in a list.
    // Note: The owning list is not stored in the node
    // so that the owning LinkedList does not have to be pinned.
    pub(crate) owned: AtomicBool,
    pub(crate) next: Cell<Option<NonNull<Node<T, N>>>>,
    pub(crate) prev: Cell<Option<NonNull<Node<T, N>>>>,
    _pin: PhantomPinned,
    _phantom: PhantomData<(fn(Node<T, N>) -> Node<T, N>, N)>,
}

#[allow(dead_code)]
impl<T: LinkedListNode<N>, N: LinkedListTag> Node<T, N> {
    pub const fn new() -> Node<T, N> {
        Node {
            owned: AtomicBool::new(false),
            next: Cell::new(None),
            prev: Cell::new(None),
            _pin: PhantomPinned,
            _phantom: PhantomData,
        }
    }

    fn get_item<'item>(&self) -> Pin<&'item T> {
        let node_ptr = self as *const Node<T, N>;
        let node_offset = <T as LinkedListNode<N>>::node_offset();
        let item_ptr = unsafe { (node_ptr as *const u8).sub(node_offset) as *const T };
        unsafe { Pin::new_unchecked(&*item_ptr) }
    }

    fn next_node(&self) -> Option<&Node<T, N>> {
        self.next.get().map(|node_ptr| unsafe { node_ptr.as_ref() })
    }

    fn prev_node(&self) -> Option<&Node<T, N>> {
        self.prev.get().map(|node_ptr| unsafe { node_ptr.as_ref() })
    }

    pub fn in_list(&self) -> bool {
        self.owned.load(Ordering::Relaxed)
    }

    fn as_ptr(&self) -> NonNull<Node<T, N>> {
        unsafe { NonNull::new_unchecked(self as *const Node<T, N> as *mut Node<T, N>) }
    }
}

#[allow(dead_code)]
pub(crate) trait LinkedListNode<N: LinkedListTag>
where
    Self: Sized,
{
    fn get_node<'item>(&'item self) -> &'item Node<Self, N>;

    fn get_node_ptr(&self) -> NonNull<Node<Self, N>> {
        let node = self.get_node();
        NonNull::from(node)
    }

    fn node_offset() -> usize;

    fn get_node_from_ptr<'node>(ptr: NonNull<Self>) -> &'node Node<Self, N> {
        let node = unsafe { ptr.as_ref() };
        node.get_node()
    }

    fn in_list(&self) -> bool {
        self.get_node().in_list()
    }
}

macro_rules! impl_linked {
    ($node_name:ident, $t:ty, $n:ty) => {
        impl $crate::kernel::list::LinkedListNode<$n> for $t {
            fn get_node<'item>(&'item self) -> &'item $crate::kernel::list::Node<Self, $n> {
                &self.$node_name
            }

            fn node_offset() -> usize {
                ::core::mem::offset_of!($t, $node_name)
            }
        }
    };
}

pub(crate) use impl_linked;

#[allow(dead_code)]
pub(crate) struct Cursor<'list, T: LinkedListNode<N>, N: LinkedListTag> {
    current: Option<NonNull<Node<T, N>>>,
    list: Pin<&'list LinkedList<T, N>>,
}

#[allow(dead_code)]
impl<'list, T: LinkedListNode<N>, N: LinkedListTag> Cursor<'list, T, N> {
    pub fn move_next(&mut self) {
        match self.current.take() {
            Some(ptr) => {
                self.current = unsafe { ptr.as_ref() }.next.get();
            }
            None => {
                self.current = None;
            }
        }
    }

    pub fn move_prev(&mut self) {
        match self.current.take() {
            Some(ptr) => {
                self.current = unsafe { ptr.as_ref() }.prev.get();
            }
            None => {
                self.current = None;
            }
        }
    }

    pub fn get_item<'item>(&self) -> Option<Pin<&'item T>> {
        self.current
            .map(|current| unsafe { current.as_ref().get_item() })
    }
}

impl<'list, T: LinkedListNode<N>, N: LinkedListTag> Iterator for Cursor<'list, T, N> {
    type Item = Pin<&'list T>;
    fn next(&mut self) -> Option<Self::Item> {
        let item = self.get_item();
        self.move_next();
        item
    }
}

impl<'list, T: LinkedListNode<N>, N: LinkedListTag> DoubleEndedIterator for Cursor<'list, T, N> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let item = self.get_item();
        self.move_prev();
        item
    }
}

pub(crate) struct CursorMut<'list, T: LinkedListNode<N>, N: LinkedListTag> {
    current: Option<NonNull<Node<T, N>>>,
    list: Pin<&'list mut LinkedList<T, N>>,
}

#[allow(dead_code)]
impl<'list, T: LinkedListNode<N>, N: LinkedListTag> CursorMut<'list, T, N> {
    pub fn insert_before<'item>(&mut self, item: Pin<&'item T>) {
        let node_link_ptr = item.get_node_ptr();
        match self.current {
            Some(current_ptr) => {
                let current_node = unsafe { current_ptr.as_ref() };
                let maybe_prev = current_node.prev.get();

                match maybe_prev {
                    Some(prev_ptr) => {
                        let prev_node = unsafe { prev_ptr.as_ref() };
                        prev_node.next.set(Some(node_link_ptr));
                        item.get_node().prev.set(Some(prev_ptr));
                        item.get_node().next.set(Some(current_ptr));
                        current_node.prev.set(Some(node_link_ptr));
                        current_node.owned.store(true, Ordering::Relaxed);
                    }
                    None => {
                        self.list.as_mut().push_front(item);
                    }
                }
            }
            None => {
                // If at "ghost" node, insert at tail
                self.list.as_mut().push_back(item)
            }
        }
    }

    pub fn insert_after<'item>(&mut self, node: Pin<&'item T>) {
        let node_ptr = node.get_node_ptr();
        match self.current {
            Some(current_ptr) => {
                let current_node = unsafe { current_ptr.as_ref() };
                let maybe_next = current_node.next.get();

                match maybe_next {
                    Some(next_ptr) => {
                        let next_node = unsafe { next_ptr.as_ref() };
                        next_node.prev.set(Some(node_ptr));
                        node.get_node().prev.set(Some(current_ptr));
                        node.get_node().next.set(Some(next_ptr));
                        current_node.next.set(Some(node_ptr));
                        current_node.owned.store(true, Ordering::Relaxed);
                    }
                    None => {
                        self.list.as_mut().push_back(node);
                    }
                }
            }
            None => self.list.as_mut().push_front(node),
        }
    }

    pub fn move_next(&mut self) {
        match self.current.take() {
            Some(ptr) => {
                self.current = unsafe { ptr.as_ref() }.next.get();
            }
            None => {
                self.current = self.list.head;
            }
        }
    }

    pub fn move_prev(&mut self) {
        match self.current.take() {
            Some(ptr) => {
                self.current = unsafe { ptr.as_ref() }.prev.get();
            }
            None => {
                self.current = self.list.tail;
            }
        }
    }

    #[allow(dead_code)]
    pub fn get_item<'item>(&self) -> Option<Pin<&'item T>> {
        self.current
            .map(|current| unsafe { current.as_ref() }.get_item())
    }
}

#[cfg(test)]
mod test {
    use super::{LinkedList, LinkedListTag, Node};
    use core::pin::{Pin, pin};

    struct Tag0 {}

    impl LinkedListTag for Tag0 {}

    struct Tag1 {}

    impl LinkedListTag for Tag1 {}

    struct Foo {
        anode: Node<Foo, Tag0>,
        bnode: Node<Foo, Tag1>,
    }

    impl_linked!(anode, Foo, Tag0);
    impl_linked!(bnode, Foo, Tag1);

    #[test_case]
    fn test_linked_list_1() {
        let mut list = pin!(LinkedList::<Foo, Tag0>::new());
        let a = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });

        list.as_mut().push_front(a.as_ref());
        let maybe_a = list.as_mut().pop_front();
        assert!(&*maybe_a.unwrap() as *const Foo == &*a as *const Foo);

        list.as_mut().push_front(a.as_ref());
        let maybe_a = list.pop_front();
        assert!(&*maybe_a.unwrap() as *const Foo == &*a as *const Foo);
    }

    #[test_case]
    fn test_linked_list_2() {
        let mut list = pin!(LinkedList::<Foo, Tag0>::new());
        let a = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        let b = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });

        list.as_mut().push_front(a.as_ref());
        list.as_mut().push_front(b.as_ref());
        let maybe_b = list.as_mut().pop_front();
        assert!(&*maybe_b.unwrap() as *const Foo == &*b as *const Foo);
        let maybe_a = list.pop_front();
        assert!(&*maybe_a.unwrap() as *const Foo == &*a as *const Foo);
    }

    #[test_case]
    fn test_linked_list_3() {
        let mut list = pin!(LinkedList::<Foo, Tag0>::new());
        let mut a = Foo {
            anode: Node::new(),
            bnode: Node::new(),
        };
        let mut b = Foo {
            anode: Node::new(),
            bnode: Node::new(),
        };
        let a = pin!(a);
        let b = pin!(b);

        list.as_mut().push_back(a.as_ref());
        list.as_mut().push_back(b.as_ref());
        let maybe_a = list.as_mut().pop_front();
        assert!(&*maybe_a.unwrap() as *const Foo == &*a as *const Foo);
        let maybe_b = list.pop_front();
        assert!(&*maybe_b.unwrap() as *const Foo == &*b as *const Foo);
    }

    #[test_case]
    fn test_linked_list_4() {
        let mut list = pin!(LinkedList::<Foo, Tag0>::new());
        let a = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        let b = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        let c = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        list.as_mut().push_back(a.as_ref());
        list.as_mut().push_back(b.as_ref());
        list.as_mut().push_back(c.as_ref());

        unsafe { list.as_mut().remove(c.as_ref()) };
        let maybe_a = list.as_mut().pop_front();
        assert!(&*maybe_a.unwrap() as *const Foo == &*a as *const Foo);
        let maybe_b = list.pop_front();
        assert!(&*maybe_b.unwrap() as *const Foo == &*b as *const Foo);
    }

    #[test_case]
    fn test_linked_list_5() {
        let mut list = pin!(LinkedList::<Foo, Tag0>::new());
        let a = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        let b = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        let c = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        list.as_mut().push_back(a.as_ref());
        list.as_mut().push_back(b.as_ref());
        list.as_mut().push_back(c.as_ref());

        unsafe { list.as_mut().remove(a.as_ref()) };
        let maybe_b = list.as_mut().pop_front();
        assert!(&*maybe_b.unwrap() as *const Foo == &*b as *const Foo);
        let maybe_c = list.pop_front();
        assert!(&*maybe_c.unwrap() as *const Foo == &*c as *const Foo);
    }

    #[test_case]
    fn test_linked_list_5() {
        let mut list = pin!(LinkedList::<Foo, Tag0>::new());
        let a = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        let b = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        let c = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });
        list.as_mut().push_back(a.as_ref());
        list.as_mut().push_back(b.as_ref());
        list.as_mut().push_back(c.as_ref());

        unsafe { list.as_mut().remove(b.as_ref()) };
        let maybe_a = list.as_mut().pop_front();
        assert!(&*maybe_a.unwrap() as *const Foo == &*a as *const Foo);
        let maybe_c = list.pop_front();
        assert!(&*maybe_c.unwrap() as *const Foo == &*c as *const Foo);
    }

    #[test_case]
    fn test_linked_list_5() {
        let mut list = pin!(LinkedList::<Foo, Tag0>::new());
        let a = pin!(Foo {
            anode: Node::new(),
            bnode: Node::new(),
        });

        list.as_mut().push_back(a.as_ref());

        unsafe { list.as_mut().remove(a.as_ref()) };
        assert!(list.as_mut().pop_front().is_none());
    }
}
