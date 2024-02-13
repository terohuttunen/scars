use core::cell::Cell;
use core::marker::PhantomData;
use core::marker::PhantomPinned;
use core::ops::Deref;
use core::ptr::NonNull;

pub trait LinkedListTag: 'static {}

pub(crate) struct LinkedList<T, N: LinkedListTag> {
    len: usize,
    head: Option<NonNull<Link<T, N>>>,
    tail: Option<NonNull<Link<T, N>>>,
    _phantom: PhantomData<(Cell<Link<T, N>>, N)>,
    // Links have backreferences to containing lists.
    // Therefore, lists must be pinned.
    _pinned: PhantomPinned,
}

#[allow(dead_code)]
impl<T: Linked<N>, N: LinkedListTag> LinkedList<T, N> {
    pub const fn new() -> LinkedList<T, N> {
        LinkedList {
            len: 0,
            head: None,
            tail: None,
            _phantom: PhantomData,
            _pinned: PhantomPinned,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn head<'item>(&self) -> Option<&'item T> {
        self.head
            .map(|link_ptr| unsafe { link_ptr.as_ref().get_item() })
    }

    pub fn tail<'item>(&self) -> Option<&'item T> {
        self.tail
            .map(|link_ptr| unsafe { link_ptr.as_ref().get_item() })
    }

    pub fn push_front<'item>(&mut self, item: &'item T) {
        let link = item.get_link();
        let link_ptr = item.get_link_ptr();

        // When node is pushed to a list it should not be a member of
        // any other list.
        assert!(link.next.get().is_none());
        assert!(link.prev.get().is_none());

        // Adjust old head links
        if let Some(head) = self.head() {
            let head_link = head.get_link();
            head_link.prev.set(Some(link_ptr));
        }

        // Adjust new node links
        link.next.set(self.head);
        link.prev.set(None);

        // Replace head with new node
        self.head = Some(link_ptr);
        if self.tail.is_none() {
            // Replace also tail if list was empty
            self.tail = Some(link_ptr);
        }

        link.list.set(NonNull::new(self));

        self.len += 1;
    }

    pub fn push_back<'item>(&mut self, item: &'item T) {
        let link = item.get_link();
        let link_ptr = item.get_link_ptr();

        // When node is pushed to a list it should not be a member of
        // any other list.
        assert!(link.next.get().is_none());
        assert!(link.prev.get().is_none());

        // Adjust old tail links
        if let Some(tail) = self.tail() {
            let tail_link = tail.get_link();
            tail_link.next.set(Some(link_ptr));
        }

        // Adjust new node links
        link.next.set(None);
        link.prev.set(self.tail);

        // Replace tail with new node
        self.tail = Some(link_ptr);
        if self.head.is_none() {
            // Replace also head if list was empty
            self.head = Some(link_ptr);
        }

        link.list.set(NonNull::new(self));

        self.len += 1;
    }

    pub fn pop_front<'item>(&mut self) -> Option<&'item T> {
        if let Some(head) = self
            .head
            .take()
            .map(|head| unsafe { head.as_ref().get_item() })
        {
            let head_link = head.get_link();
            if let Some(next_link) = head_link.next_link() {
                next_link.prev.set(None);
                self.head = Some(next_link.as_ptr());
            } else {
                self.tail = None;
            }
            head_link.next.set(None);
            head_link.prev.set(None);
            head_link.list.set(None);
            self.len -= 1;
            return Some(head);
        }

        None
    }

    pub fn cursor_front(&self) -> Cursor<'_, T, N> {
        Cursor {
            index: 0,
            current: self.head,
            list: self,
        }
    }

    pub fn cursor_front_mut(&mut self) -> CursorMut<'_, T, N> {
        CursorMut {
            index: 0,
            current: self.head,
            list: self,
        }
    }

    // Insert to list after predicate becomes false
    pub fn insert_after_condition<'item, P>(&mut self, item: &'item T, predicate: P)
    where
        P: Fn(&T, &T) -> bool,
    {
        let mut cursor = self.cursor_front_mut();
        loop {
            match cursor.get_item() {
                Some(list_item) if predicate(list_item, item) => {
                    cursor.move_next();
                }
                _ => {
                    cursor.insert_before(item);
                    break;
                }
            }
        }
    }

    pub fn remove<'item>(&mut self, item: &'item T) {
        let link = item.get_link();
        match link.list.get() {
            Some(list_ptr) => assert!(unsafe { NonNull::new_unchecked(self) } == list_ptr),
            None => {
                // Trying to remove item that is not in a list
                return;
            }
        }

        if let Some(prev_link) = link.prev_link() {
            prev_link.next.set(link.next.get());
        } else {
            self.head = link.next.get();
        }

        if let Some(next_link) = link.next_link() {
            next_link.prev.set(link.prev.get());
        } else {
            self.tail = link.prev.get();
        }

        link.next.set(None);
        link.prev.set(None);
        link.list.set(None);
        self.len -= 1;
    }
}

pub(crate) struct Link<T, N: LinkedListTag> {
    list: Cell<Option<NonNull<LinkedList<T, N>>>>,
    next: Cell<Option<NonNull<Link<T, N>>>>,
    prev: Cell<Option<NonNull<Link<T, N>>>>,
    _pin: PhantomPinned,
    _phantom: PhantomData<(fn(Link<T, N>) -> Link<T, N>, N)>,
}

impl<T: Linked<N>, N: LinkedListTag> Link<T, N> {
    pub const fn new() -> Link<T, N> {
        Link {
            list: Cell::new(None),
            next: Cell::new(None),
            prev: Cell::new(None),
            _pin: PhantomPinned,
            _phantom: PhantomData,
        }
    }

    fn get_item<'item>(&self) -> &'item T {
        let link_ptr = self as *const Link<T, N>;
        let link_offset = <T as Linked<N>>::link_offset();
        let node_ptr = unsafe { (link_ptr as *const u8).sub(link_offset) as *const T };
        unsafe { &*node_ptr }
    }

    fn next_link(&self) -> Option<&Link<T, N>> {
        self.next.get().map(|link_ptr| unsafe { link_ptr.as_ref() })
    }

    fn prev_link(&self) -> Option<&Link<T, N>> {
        self.prev.get().map(|link_ptr| unsafe { link_ptr.as_ref() })
    }

    fn in_list(&self) -> bool {
        self.list.get().is_some()
    }

    fn as_ptr(&self) -> NonNull<Link<T, N>> {
        unsafe { NonNull::new_unchecked(self as *const Link<T, N> as *mut Link<T, N>) }
    }
}

pub(crate) trait Linked<N: LinkedListTag>
where
    Self: Sized,
{
    fn get_link<'item>(&'item self) -> &'item Link<Self, N>;

    fn get_link_ptr(&self) -> NonNull<Link<Self, N>> {
        let link = self.get_link();
        NonNull::from(link)
    }

    fn link_offset() -> usize;

    fn get_link_from_ptr<'link>(ptr: NonNull<Self>) -> &'link Link<Self, N> {
        let node = unsafe { ptr.as_ref() };
        node.get_link()
    }

    fn in_list(&self) -> bool {
        self.get_link().in_list()
    }
}

macro_rules! impl_linked {
    ($link_name:ident, $t:ty, $n:ty) => {
        impl $crate::kernel::list::Linked<$n> for $t {
            fn get_link<'item>(&'item self) -> &'item $crate::kernel::list::Link<Self, $n> {
                &self.$link_name
            }

            fn link_offset() -> usize {
                ::core::mem::offset_of!($t, $link_name)
            }
        }
    };
}

pub(crate) use impl_linked;

pub(crate) struct Cursor<'list, T: Linked<N>, N: LinkedListTag> {
    index: usize,
    current: Option<NonNull<Link<T, N>>>,
    list: &'list LinkedList<T, N>,
}

#[allow(dead_code)]
impl<'list, T: Linked<N>, N: LinkedListTag> Cursor<'list, T, N> {
    pub fn move_next(&mut self) {
        match self.current.take() {
            Some(ptr) => {
                self.index += 1;
                self.current = unsafe { ptr.as_ref() }.next.get();
            }
            None => {
                self.index = 0;
                self.current = None;
            }
        }
    }

    pub fn move_prev(&mut self) {
        match self.current.take() {
            Some(ptr) => {
                self.index -= 1;
                self.current = unsafe { ptr.as_ref() }.prev.get();
            }
            None => {
                self.index = self.list.len - 1;
                self.current = None;
            }
        }
    }

    pub fn get_item<'item>(&self) -> Option<&'item T> {
        self.current
            .map(|current| unsafe { current.as_ref().get_item() })
    }

    pub fn index(&self) -> Option<usize> {
        self.current.map(|_| self.index)
    }
}

impl<'list, T: Linked<N>, N: LinkedListTag> Iterator for Cursor<'list, T, N> {
    type Item = &'list T;
    fn next(&mut self) -> Option<Self::Item> {
        let item = self.get_item();
        self.move_next();
        item
    }
}

impl<'list, T: Linked<N>, N: LinkedListTag> DoubleEndedIterator for Cursor<'list, T, N> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let item = self.get_item();
        self.move_prev();
        item
    }
}

pub(crate) struct CursorMut<'list, T: Linked<N>, N: LinkedListTag> {
    index: usize,
    current: Option<NonNull<Link<T, N>>>,
    list: &'list mut LinkedList<T, N>,
}

#[allow(dead_code)]
impl<'list, T: Linked<N>, N: LinkedListTag> CursorMut<'list, T, N> {
    pub fn insert_before<'item>(&mut self, item: &'item T) {
        let node_link_ptr = item.get_link_ptr();
        match self.current {
            Some(current_ptr) => {
                let current_link = unsafe { current_ptr.as_ref() };
                let maybe_prev = current_link.prev.get();

                match maybe_prev {
                    Some(prev_ptr) => {
                        let prev_link = unsafe { prev_ptr.as_ref() };
                        prev_link.next.set(Some(node_link_ptr));
                        item.get_link().prev.set(Some(prev_ptr));
                        item.get_link().next.set(Some(current_ptr));
                        current_link.prev.set(Some(node_link_ptr));
                        current_link.list.set(NonNull::new(self.list as *mut _));
                        self.list.len += 1;
                    }
                    None => {
                        self.list.push_front(item);
                    }
                }
                self.index += 1;
            }
            None => {
                // If at "ghost" node, insert at tail
                self.list.push_back(item)
            }
        }
    }

    pub fn insert_after<'item>(&mut self, node: &'item T) {
        let node_link_ptr = node.get_link_ptr();
        match self.current {
            Some(current_ptr) => {
                let current_link = unsafe { current_ptr.as_ref() };
                let maybe_next = current_link.next.get();

                match maybe_next {
                    Some(next_ptr) => {
                        let next_link = unsafe { next_ptr.as_ref() };
                        next_link.prev.set(Some(node_link_ptr));
                        node.get_link().prev.set(Some(current_ptr));
                        node.get_link().next.set(Some(next_ptr));
                        current_link.next.set(Some(node_link_ptr));
                        current_link.list.set(NonNull::new(self.list as *mut _));
                        self.list.len += 1;
                    }
                    None => {
                        self.list.push_back(node);
                    }
                }
            }
            None => self.list.push_front(node),
        }
    }

    pub fn move_next(&mut self) {
        match self.current.take() {
            Some(ptr) => {
                self.index += 1;
                self.current = unsafe { ptr.as_ref() }.next.get();
            }
            None => {
                self.index = 0;
                self.current = self.list.head;
            }
        }
    }

    pub fn move_prev(&mut self) {
        match self.current.take() {
            Some(ptr) => {
                self.index -= 1;
                self.current = unsafe { ptr.as_ref() }.prev.get();
            }
            None => {
                self.index = self.list.len - 1;
                self.current = self.list.tail;
            }
        }
    }

    #[allow(dead_code)]
    pub fn get_item<'item>(&mut self) -> Option<&'item T> {
        self.current
            .map(|current| unsafe { current.as_ref() }.get_item())
    }

    #[allow(dead_code)]
    pub fn index(&self) -> Option<usize> {
        self.current.map(|_| self.index)
    }
}

#[cfg(test)]
mod test {
    use super::{Link, LinkedList, LinkedListTag};

    struct Tag0 {}

    impl LinkedListTag for Tag0 {}

    struct Tag1 {}

    impl LinkedListTag for Tag1 {}

    struct Foo {
        alink: Link<Foo, Tag0>,
        blink: Link<Foo, Tag1>,
    }

    impl_linked!(alink, Foo, Tag0);
    impl_linked!(blink, Foo, Tag1);

    #[test_case]
    fn test_linked_list_1() {
        let mut list = LinkedList::<Foo, Tag0>::new();
        let mut a = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        assert_eq!(list.len(), 0);

        list.push_front(&mut a);
        assert_eq!(list.len(), 1);
        let maybe_a = list.pop_front();
        assert_eq!(list.len(), 0);
        assert!(maybe_a.unwrap() as *const Foo == &a as *const Foo);

        list.push_front(&mut a);
        assert_eq!(list.len(), 1);
        let maybe_a = list.pop_front();
        assert_eq!(list.len(), 0);
        assert!(maybe_a.unwrap() as *const Foo == &a as *const Foo);
    }

    #[test_case]
    fn test_linked_list_2() {
        let mut list = LinkedList::<Foo, Tag0>::new();
        let mut a = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        let mut b = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        assert_eq!(list.len(), 0);

        list.push_front(&mut a);
        assert_eq!(list.len(), 1);
        list.push_front(&mut b);
        assert_eq!(list.len(), 2);
        let maybe_b = list.pop_front();
        assert!(maybe_b.unwrap() as *const Foo == &b as *const Foo);
        assert_eq!(list.len(), 1);
        let maybe_a = list.pop_front();
        assert!(maybe_a.unwrap() as *const Foo == &a as *const Foo);
        assert_eq!(list.len(), 0);
    }

    #[test_case]
    fn test_linked_list_3() {
        let mut list = LinkedList::<Foo, Tag0>::new();
        let mut a = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        let mut b = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        assert_eq!(list.len(), 0);

        list.push_back(&mut a);
        assert_eq!(list.len(), 1);
        list.push_back(&mut b);
        assert_eq!(list.len(), 2);
        let maybe_a = list.pop_front();
        assert!(maybe_a.unwrap() as *const Foo == &a as *const Foo);
        assert_eq!(list.len(), 1);
        let maybe_b = list.pop_front();
        assert!(maybe_b.unwrap() as *const Foo == &b as *const Foo);
        assert_eq!(list.len(), 0);
    }

    #[test_case]
    fn test_linked_list_4() {
        let mut list = LinkedList::<Foo, Tag0>::new();
        let a = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        let b = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        let c = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        list.push_back(&a);
        list.push_back(&b);
        list.push_back(&c);

        list.remove(&c);
        let maybe_a = list.pop_front();
        assert!(maybe_a.unwrap() as *const Foo == &a as *const Foo);
        assert_eq!(list.len(), 1);
        let maybe_b = list.pop_front();
        assert!(maybe_b.unwrap() as *const Foo == &b as *const Foo);
        assert_eq!(list.len(), 0);
    }

    #[test_case]
    fn test_linked_list_5() {
        let mut list = LinkedList::<Foo, Tag0>::new();
        let a = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        let b = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        let c = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        list.push_back(&a);
        list.push_back(&b);
        list.push_back(&c);

        list.remove(&a);
        let maybe_b = list.pop_front();
        assert!(maybe_b.unwrap() as *const Foo == &b as *const Foo);
        assert_eq!(list.len(), 1);
        let maybe_c = list.pop_front();
        assert!(maybe_c.unwrap() as *const Foo == &c as *const Foo);
        assert_eq!(list.len(), 0);
    }

    #[test_case]
    fn test_linked_list_5() {
        let mut list = LinkedList::<Foo, Tag0>::new();
        let a = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        let b = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        let c = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };
        list.push_back(&a);
        list.push_back(&b);
        list.push_back(&c);

        list.remove(&b);
        let maybe_a = list.pop_front();
        assert!(maybe_a.unwrap() as *const Foo == &a as *const Foo);
        assert_eq!(list.len(), 1);
        let maybe_c = list.pop_front();
        assert!(maybe_c.unwrap() as *const Foo == &c as *const Foo);
        assert_eq!(list.len(), 0);
    }

    #[test_case]
    fn test_linked_list_5() {
        let mut list = LinkedList::<Foo, Tag0>::new();
        let a = Foo {
            alink: Link::new(),
            blink: Link::new(),
        };

        list.push_back(&a);

        list.remove(&a);
        assert!(list.pop_front().is_none());
        assert_eq!(list.len(), 0);
    }
}
