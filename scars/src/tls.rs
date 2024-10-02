//! Thread Local Storage
use crate::kernel::{scheduler::ExecutionContext, Scheduler};
use core::any::TypeId;
use core::cell::Cell;
use static_cell::{ConstStaticCell, StaticCell};

#[macro_export]
macro_rules! make_local_storage {
    ( $size:expr ) => {{
        static LOCAL_STORAGE: $crate::tls::LocalStorageArray<$size> =
            $crate::tls::LocalStorageArray::new();
        LOCAL_STORAGE.take()
    }};
}

pub struct LocalStorage(&'static [LocalStorageEntry]);

impl LocalStorage {
    fn find_entry(&self, key: Option<TypeId>) -> Option<&'static LocalStorageEntry> {
        self.0.iter().find(|entry| entry.key.get() == key)
    }

    fn remove_entry(&self, key: TypeId) {
        if let Some(entry) = self.find_entry(Some(key)) {
            entry.key.set(None);
            entry.value.set(core::ptr::null_mut());
        }
    }

    fn get_from_current_tls(key: TypeId) -> Option<&'static LocalStorageEntry> {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(interrupt) => interrupt
                .local_storage
                .get()
                .map(|tls| tls.find_entry(Some(key)))?,
            ExecutionContext::Thread(thread) => thread
                .local_storage
                .get()
                .map(|tls| tls.find_entry(Some(key)))?,
        }
    }

    fn put_by_type_id(&self, key: TypeId, value_ptr: *mut ()) {
        let unused_entry = self.find_entry(None).unwrap();
        unused_entry.key.set(Some(key));
        unused_entry.value.set(value_ptr);
    }

    fn put_to_current_tls(key: TypeId, value_ptr: *mut ()) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(interrupt) => interrupt
                .local_storage
                .get()
                .expect("Interrupt does not have local storage")
                .put_by_type_id(key, value_ptr),
            ExecutionContext::Thread(thread) => thread
                .local_storage
                .get()
                .expect("Thread does not have local storage")
                .put_by_type_id(key, value_ptr),
        }
    }

    pub fn get<T: 'static>() -> Option<&'static T> {
        let key = TypeId::of::<T>();
        Self::get_from_current_tls(key).map(|entry| unsafe { &*(entry.value.get() as *const T) })
    }

    pub fn put<T: 'static>(local_data: &'static ConstLocalCell<T>) {
        let key = TypeId::of::<T>();
        Self::put_to_current_tls(key, local_data.take() as *mut _ as *mut ());
    }

    pub fn put_with<T: 'static, R>(
        local_data: &'static ConstLocalCell<T>,
        on_put: impl FnOnce(&mut T) -> R,
    ) -> R {
        let key = TypeId::of::<T>();
        let local_data = local_data.take();
        let rval = on_put(local_data);
        Self::put_to_current_tls(key, local_data as *mut _ as *mut ());
        rval
    }

    pub fn put_init<T: 'static>(local_data: &'static LocalCell<T>, val: T) {
        let key = TypeId::of::<T>();
        let local_data = local_data.init(val);
        Self::put_to_current_tls(key, local_data as *mut _ as *mut ());
    }

    pub fn put_init_with<T: 'static>(local_data: &'static LocalCell<T>, init: impl FnOnce() -> T) {
        let key = TypeId::of::<T>();
        let local_data = local_data.init_with(init);
        Self::put_to_current_tls(key, local_data as *mut _ as *mut ());
    }

    pub unsafe fn raw_get<T: 'static>(&self) -> Option<&'static T> {
        let key = TypeId::of::<T>();
        self.find_entry(Some(key))
            .map(|entry| unsafe { &*(entry.value.get() as *const T) })
    }

    pub fn raw_put<T: 'static>(&mut self, local_data: &'static ConstLocalCell<T>) -> &mut Self {
        let key = TypeId::of::<T>();
        self.put_by_type_id(key, local_data.take() as *mut _ as *mut ());
        self
    }

    pub fn raw_put_with<T: 'static, R>(
        &mut self,
        local_data: &'static ConstLocalCell<T>,
        on_put: impl FnOnce(&mut T) -> R,
    ) -> R {
        let key = TypeId::of::<T>();
        let local_data = local_data.take();
        let rval = on_put(local_data);
        self.put_by_type_id(key, local_data as *mut _ as *mut ());
        rval
    }

    pub fn raw_put_init<T: 'static>(
        &mut self,
        local_data: &'static LocalCell<T>,
        val: T,
    ) -> &mut Self {
        let key = TypeId::of::<T>();
        let local_data = local_data.init(val);
        self.put_by_type_id(key, local_data as *mut _ as *mut ());
        self
    }

    pub fn raw_put_init_with<T: 'static>(
        &mut self,
        local_data: &'static LocalCell<T>,
        init: impl FnOnce() -> T,
    ) -> &mut Self {
        let key = TypeId::of::<T>();
        let local_data = local_data.init_with(init);
        self.put_by_type_id(key, local_data as *mut _ as *mut ());
        self
    }

    pub fn remove<T: 'static>() {
        let key = TypeId::of::<T>();
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(interrupt) => {
                interrupt
                    .local_storage
                    .get()
                    .map(|tls| tls.remove_entry(key));
            }
            ExecutionContext::Thread(thread) => {
                thread.local_storage.get().map(|tls| tls.remove_entry(key));
            }
        }
    }
}

unsafe impl Sync for LocalStorage {}

#[derive(Clone)]
pub struct LocalStorageEntry {
    key: Cell<Option<TypeId>>,
    value: Cell<*mut ()>,
}

const INITIAL_ENTRY: LocalStorageEntry = LocalStorageEntry {
    key: Cell::new(None),
    value: Cell::new(core::ptr::null_mut()),
};

pub struct LocalStorageArray<const N: usize> {
    entries: ConstStaticCell<[LocalStorageEntry; N]>,
}

impl<const N: usize> LocalStorageArray<N> {
    pub const fn new() -> LocalStorageArray<N> {
        LocalStorageArray {
            entries: ConstStaticCell::new([INITIAL_ENTRY; N]),
        }
    }

    pub fn take(&'static self) -> LocalStorage {
        LocalStorage(self.entries.take())
    }
}

unsafe impl<const N: usize> Sync for LocalStorageArray<N> {}

#[repr(transparent)]
pub struct ConstLocalCell<T: 'static> {
    data: ConstStaticCell<T>,
}

impl<T: 'static> ConstLocalCell<T> {
    pub const fn new(data: T) -> ConstLocalCell<T> {
        ConstLocalCell {
            data: ConstStaticCell::new(data),
        }
    }

    pub fn take(&'static self) -> &'static mut T {
        self.data.take()
    }
}

impl<T: 'static> Drop for ConstLocalCell<T> {
    fn drop(&mut self) {
        LocalStorage::remove::<T>();
    }
}

// SAFETY: ConstLocalCell is safe to Sync because the data it contains
// can be accessed only through the LocalStorage API, which lets
// only the current thread to access the data.
unsafe impl<T> Sync for ConstLocalCell<T> {}

#[repr(transparent)]
pub struct LocalCell<T: 'static> {
    data: StaticCell<T>,
}

impl<T: 'static> LocalCell<T> {
    pub const fn new() -> LocalCell<T> {
        LocalCell {
            data: StaticCell::new(),
        }
    }

    pub fn init(&'static self, val: T) -> &'static mut T {
        self.data.init(val)
    }

    pub fn init_with(&'static self, init: impl FnOnce() -> T) -> &'static mut T {
        self.data.init_with(init)
    }
}

impl<T: 'static> Drop for LocalCell<T> {
    fn drop(&mut self) {
        LocalStorage::remove::<T>();
    }
}

// SAFETY: LocalCell is safe to Sync because the data it contains
// can be accessed only through the LocalStorage API, which lets
// only the current thread to access the data.
unsafe impl<T> Sync for LocalCell<T> {}
