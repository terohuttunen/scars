#![feature(sync_unsafe_cell)]
#![feature(linkage)]
extern crate libc;
extern crate std;

use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::AtomicBool;
use scars_khal::*;

mod context;
mod defmt;
mod error;
mod flow;
mod interrupt;
pub mod pac;
mod signal;
mod timer;

pub use ::defmt::println as printk;
pub use ::defmt::println as printkln;

pub use context::{VirtualContext, VirtualTrap};
pub use error::{SimContext, SimulatorError, SimulatorErrorKind};
pub use interrupt::{
    InterruptClaim as SimInterruptClaim, MAX_INTERRUPT, MAX_INTERRUPT_PRIORITY,
    VirtualInterruptController, pend_interrupt,
};
pub use timer::{TIMER_FREQ_HZ, VirtualTimer};

pub type HAL = Simulator;

// Static HAL instance using SyncUnsafeCell directly
static HAL: SyncUnsafeCell<MaybeUninit<Simulator>> = SyncUnsafeCell::new(MaybeUninit::uninit());

pub struct Simulator {
    timer: VirtualTimer,
    interrupt_controller: VirtualInterruptController,
    service_call_pending: AtomicBool,
}

unsafe impl Sync for Simulator {}

impl HardwareAbstractionLayer for Simulator {
    const NAME: &'static str = "Simulator";

    fn instance() -> &'static Self {
        unsafe { (&*HAL.get()).assume_init_ref() }
    }

    unsafe fn init(hal: *mut Self) {
        unsafe {
            VirtualTimer::init(&raw mut (*hal).timer);
            (*hal).interrupt_controller = VirtualInterruptController::new();
            (*hal).service_call_pending = AtomicBool::new(false);
        }
    }
}

#[unsafe(no_mangle)]
fn main() {
    unsafe {
        start_kernel();
    }
}

#[unsafe(no_mangle)]
#[linkage = "weak"]
fn _scars_idle_thread_hook() {
    Simulator::on_idle();
}
