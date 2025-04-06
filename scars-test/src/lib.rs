#![cfg_attr(not(feature = "std"), no_std)]
use core::panic::PanicInfo;

#[cfg(feature = "semihosting")]
use semihosting::{print, println};

#[cfg(feature = "rtt")]
use rtt_target::{rprint as print, rprintln as println};

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

#[macro_export]
macro_rules! integration_test {
    () => {
        #[cfg_attr(
            not(feature = "khal-sim"),
            ::scars::entry(name = "main", priority = 1, stack_size = 1024)
        )]
        #[cfg_attr(
            feature = "khal-sim",
            ::scars::entry(name = "main", priority = 1, stack_size = 16384)
        )]
        pub fn main() {
            test_main();
            scars_test::test_succeed();
        }
    };
}
