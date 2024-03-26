pub use ::core::fmt::{Error, Write};
use core::cell::SyncUnsafeCell;

pub struct Printk {}

impl ::core::fmt::Write for Printk {
    fn write_str(&mut self, s: &str) -> Result<(), Error> {
        use std::io::Error;
        use std::io::Write;
        use std::os::unix::io::FromRawFd;
        let mut f = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(1) });
        let result = f
            .write_all(s.as_bytes())
            .map_err(|_| ::core::fmt::Error::default())?;
        Ok(())
    }
}

#[macro_export]
macro_rules! printk {
    ($($arg:tt)*) => {{
        use ::core::fmt::Write;
        let _ = write!(unsafe {&mut *$crate::printk::PRINTK.get()}, $($arg)*);
    }};
}

#[macro_export]
macro_rules! printkln {
    () => ($crate::printk!("\r\n"));
    ($fmt:expr) => ({
        $crate::printk!(concat!($fmt, "\r\n"))
    });
    ($fmt:expr, $($arg:tt)*) => ({
        $crate::printk!(concat!($fmt, "\r\n"), $($arg)*)
    });
}

pub static PRINTK: SyncUnsafeCell<Printk> = SyncUnsafeCell::new(Printk {});
