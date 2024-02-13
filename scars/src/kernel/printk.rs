#[cfg(feature = "hal-std")]
pub extern crate std;
pub use ::core::fmt::{Error, Write};
use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;

pub struct Printk {}

#[cfg(not(feature = "hal-std"))]
impl ::core::fmt::Write for Printk {
    fn write_str(&mut self, s: &str) -> Result<(), Error> {
        use semihosting::io;
        use semihosting::io::Write;
        let mut stdout = io::stdout().unwrap();
        let _ = stdout.write_all(s.as_bytes());
        Ok(())
    }
}

#[cfg(feature = "hal-std")]
impl ::core::fmt::Write for Printk {
    fn write_str(&mut self, s: &str) -> Result<(), Error> {
        use crate::kernel::printk::std::io::Write;
        use std::io::Error;
        use std::os::unix::io::FromRawFd;
        let mut f = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(1) });
        let result = f
            .write_all(s.as_bytes())
            .map_err(|_| ::core::fmt::Error::default())?;
        Ok(())
    }
}

pub static PRINTK: SyncUnsafeCell<MaybeUninit<Printk>> = SyncUnsafeCell::new(MaybeUninit::uninit());

#[cfg(not(feature = "hal-std"))]
#[macro_export]
macro_rules! printk {
    ($($arg:tt)*) => {{
        use ::core::fmt::Write;
        let _ = write!(unsafe {(&mut *$crate::kernel::printk::PRINTK.get()).assume_init_mut()}, $($arg)*);
    }};
}

#[cfg(feature = "hal-std")]
#[macro_export]
macro_rules! printk {
    ($($arg:tt)*) => {{
        use ::core::fmt::Write;
        let _ = write!(unsafe {(&mut *$crate::kernel::printk::PRINTK.get()).assume_init_mut()}, $($arg)*);
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

pub fn init() {
    unsafe {
        let _ = (&mut *PRINTK.get()).write(Printk {});
    }
}
