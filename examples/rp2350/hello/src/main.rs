//! 1 Hz defmt heartbeat + GPIO25 LED toggle — smoke test for the RP2350 port.
//!
//! Tested on Raspberry Pi Pico 2. The onboard LED is wired to GPIO25, so
//! the toggle is visible without a probe; defmt heartbeat is streamed
//! over RTT via probe-rs when a probe is attached.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::khal::pac;
use scars::prelude::*;
use scars::thread::ThreadFn;
use scars::time::{Duration, Instant};

const HEARTBEAT_PRIORITY: Priority = Priority::thread(1);
const STACK_SIZE: usize = 1024;
/// Pico 2 onboard LED is wired to GPIO25.
const LED_GPIO: usize = 25;
const LED_MASK: u32 = 1 << LED_GPIO;

type HeartbeatF = impl ThreadFn;

/// Bring GPIO25 up as a SIO output driving the onboard LED. RP2350 holds
/// every pad in an isolation latch (PADS_BANK0.GPIOn[ISO]) after reset;
/// it must be cleared before the pad will drive anything.
fn init_led() {
    let resets = unsafe { &*pac::RESETS::ptr() };
    resets
        .reset()
        .modify(|_, w| w.io_bank0().clear_bit().pads_bank0().clear_bit());
    while resets.reset_done().read().io_bank0().bit_is_clear() {}
    while resets.reset_done().read().pads_bank0().bit_is_clear() {}

    let pads = unsafe { &*pac::PADS_BANK0::ptr() };
    pads.gpio(LED_GPIO).write(|w| {
        w.iso().clear_bit()
            .od().clear_bit()
            .pde().clear_bit()
    });

    let io = unsafe { &*pac::IO_BANK0::ptr() };
    io.gpio(LED_GPIO).gpio_ctrl().write(|w| w.funcsel().sio());

    let sio = unsafe { &*pac::SIO::ptr() };
    sio.gpio_oe_set().write(|w| unsafe { w.bits(LED_MASK) });
    // Drive high immediately so a successful boot is visible before the
    // first heartbeat tick.
    sio.gpio_out_set().write(|w| unsafe { w.bits(LED_MASK) });
}

fn toggle_led() {
    let sio = unsafe { &*pac::SIO::ptr() };
    sio.gpio_out_xor().write(|w| unsafe { w.bits(LED_MASK) });
}

#[scars::init]
#[define_opaque(HeartbeatF)]
fn init() {
    static STACK: Stack<STACK_SIZE> = Stack::new();
    static THREAD: Thread<HEARTBEAT_PRIORITY, HeartbeatF> = Thread::new("heartbeat");
    let _ = THREAD
        .init(STACK.init())
        .attach(|| {
            init_led();
            let mut count: u32 = 0;
            loop {
                scars::printkln!("[heartbeat] tick {}", count);
                toggle_led();
                count = count.wrapping_add(1);
                scars::delay_until(Instant::now() + Duration::from_secs(1));
            }
        })
        .start();
}
