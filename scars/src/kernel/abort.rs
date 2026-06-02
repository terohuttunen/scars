use crate::kernel::hal;
use scars_fault::Fault;
use scars_khal::CoreController;

#[unsafe(no_mangle)]
pub extern "C" fn abort() -> ! {
    #[cfg(all(test, not(any(feature = "khal-sim", feature = "khal-test"))))]
    semihosting::process::exit(1);
    #[cfg(any(not(test), feature = "khal-sim", feature = "khal-test"))]
    hal::exit(1)
}
