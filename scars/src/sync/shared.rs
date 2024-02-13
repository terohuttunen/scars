use crate::sync::mutex::Mutex;
use crate::AnyPriority;
use core::ops::Deref;

#[macro_export]
macro_rules! make_shared {
    ($prio:expr, $s:expr) => {{
        type T = impl ::core::marker::Sized;
        static SHARED: $crate::sync::Mutex<T, { $prio }> = $crate::sync::Mutex::new($s);

        $crate::sync::Shared::new(&SHARED)
    }};
}

pub struct Shared<T: 'static, const CEILING: AnyPriority> {
    shared: &'static Mutex<T, CEILING>,
}

impl<T: 'static, const CEILING: AnyPriority> Shared<T, CEILING> {
    pub fn new(shared: &'static Mutex<T, CEILING>) -> Shared<T, CEILING> {
        Shared { shared }
    }
}

impl<T: 'static, const CEILING: AnyPriority> Clone for Shared<T, CEILING> {
    fn clone(&self) -> Shared<T, CEILING> {
        *self
    }
}

impl<T: 'static, const CEILING: AnyPriority> Copy for Shared<T, CEILING> {}

impl<T: 'static, const CEILING: AnyPriority> Deref for Shared<T, CEILING> {
    type Target = Mutex<T, CEILING>;

    fn deref(&self) -> &Self::Target {
        self.shared
    }
}
