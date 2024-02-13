#![cfg_attr(not(feature = "std"), no_std)]
use core::panic::PanicInfo;
//use minicov::{capture_coverage, CoverageWriter};
#[cfg(not(feature = "std"))]
use semihosting::{
    fs::{write, File},
    print, println,
    process::exit,
};
#[cfg(feature = "std")]
use std::process::exit;

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

pub fn test_runner(tests: &[&dyn ScarsTest]) {
    println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }

    exit(0)
}

pub fn test_succeed() -> ! {
    println!("[ok]");
    exit(0)
}

pub fn test_fail() -> ! {
    println!("[failed]");
    exit(1)
}

#[macro_export]
macro_rules! integration_test {
    () => {
        use ::scars::kernel::exception::Exception;
        use ::scars_hal::ExceptionInfo;
        #[no_mangle]
        fn _user_exception_handler(exception: Exception) {
            match exception {
                Exception::Panic(info) => {
                    scars::printkln!("{}", info);
                }
                Exception::RuntimeError(rte) => {
                    scars::printkln!("Runtime error: {:?}", rte);
                }
                Exception::HardwareException(hwe) => {
                    scars::printkln!(
                        "HW exception: {:?} {:?} {:?}",
                        hwe.code(),
                        hwe.name(),
                        hwe.address()
                    );
                }
            }
            $crate::test_fail();
        }

        #[cfg_attr(
            not(feature = "hal-std"),
            ::scars::entry(name = "main", priority = 1, stack_size = 1024)
        )]
        #[cfg_attr(
            feature = "hal-std",
            ::scars::entry(name = "main", priority = 1, stack_size = 16384)
        )]
        pub fn main() {
            test_main();
        }
    };
}
