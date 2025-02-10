use crate::kernel::hal;
use scars_khal::FlowController;

#[unsafe(no_mangle)]
pub fn abort() -> ! {
    #[cfg(all(test, not(feature = "khal-sim")))]
    semihosting::process::exit(1);
    #[cfg(any(not(test), feature = "khal-sim"))]
    hal::abort()
}
