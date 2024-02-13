use crate::kernel::hal;
use scars_hal::FlowController;

#[no_mangle]
pub fn abort() -> ! {
    #[cfg(all(test, not(feature = "hal-std")))]
    semihosting::process::exit(1);
    #[cfg(any(not(test), feature = "hal-std"))]
    hal::abort()
}
