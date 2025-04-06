use crate::kernel::hal;
use scars_khal::FlowController;
use unrecoverable_error::UnrecoverableError;

#[unsafe(no_mangle)]
pub extern "C" fn abort() -> ! {
    #[cfg(all(test, not(feature = "khal-sim")))]
    semihosting::process::exit(1);
    #[cfg(any(not(test), feature = "khal-sim"))]
    hal::exit(1)
}
