#![cfg_attr(not(feature = "std"), no_std)]
#[allow(unused_imports)]
use core::panic::PanicInfo;

#[cfg(not(any(feature = "std", feature = "semihosting", feature = "rtt")))]
compile_error!(
    "scars-test requires one of the output backend features: `std`, `semihosting`, or `rtt`"
);

#[cfg(feature = "semihosting")]
use semihosting::{print, println};

#[cfg(feature = "rtt")]
use defmt::println as print;
#[cfg(feature = "rtt")]
use defmt::println;
#[cfg(feature = "rtt")]
use defmt_rtt as _;

unsafe extern "Rust" {
    unsafe fn exit_scars(exit_code: i32) -> !;
}

pub trait ScarsTest {
    fn run(&self);

    fn name(&self) -> &'static str;
}

impl<T> ScarsTest for T
where
    T: Fn(),
{
    fn run(&self) {
        print!("{}...\t", self.name());
        self();
        println!("[ok]");
    }

    fn name(&self) -> &'static str {
        core::any::type_name::<T>()
    }
}

pub fn test_runner(tests: &[&dyn ScarsTest]) -> ! {
    println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }

    unsafe { exit_scars(0) }
}

pub fn test_succeed() -> ! {
    println!("[ok]");
    unsafe { exit_scars(0) }
}

pub fn test_fail() -> ! {
    println!("[failed]");
    unsafe { exit_scars(1) }
}
