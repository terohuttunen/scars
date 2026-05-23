//! Cross-core thread start smoke test for RP2350.
//!
//! `#[scars::init]` runs on core 0. After bringing up GPIO25 (the
//! Pico 2 onboard LED), it starts two threads: one bound to core 0
//! that emits a 1 Hz defmt heartbeat, and one bound to core 1 that
//! toggles the LED at 2 Hz. The LED toggle is visible without any
//! debug probe — if you flashed with `picotool` and the LED blinks,
//! core 1 successfully started over the SIO inter-core IPI.
//!
//! Expected output:
//! - LED: blinks at ~1 Hz (toggled every 500 ms by core 1).
//! - defmt (probe-rs only): `[core0] tick 0`, `[core0] tick 1`, …
//!
//! Build for picotool:
//! ```
//! cargo xtask build --board pico-2 --example multicore_start
//! picotool load -t elf \
//!   examples/rp2350/multicore_start/target/thumbv8m.main-none-eabihf/release/multicore_start
//! picotool reboot
//! ```
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::khal::pac;
use scars::prelude::*;
use scars::thread::ThreadFn;
use scars::time::{Duration, Instant};
use scars::CoreId;

const PRIO: Priority = Priority::thread(2);
const STACK_SIZE: usize = 1024;

/// Pico 2 onboard LED.
const LED_GPIO: usize = 25;
const LED_MASK: u32 = 1 << LED_GPIO;

type Core0F = impl ThreadFn;
type Core1F = impl ThreadFn;

/// Bring GPIO25 up as a SIO output driving the onboard LED. RP2350
/// holds every pad in an isolation latch after reset; it must be
/// cleared before the pad will drive anything. Runs once on core 0
/// during `init`, before either thread is started.
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
    // Start off so the first toggle is visibly an "on".
    sio.gpio_out_clr().write(|w| unsafe { w.bits(LED_MASK) });
}

/// Atomically flip the LED via SIO's per-bit XOR alias. Safe to call
/// from any core — SIO writes are single-cycle and don't RMW.
fn toggle_led() {
    let sio = unsafe { &*pac::SIO::ptr() };
    sio.gpio_out_xor().write(|w| unsafe { w.bits(LED_MASK) });
}

/// Core 0's body: 1 Hz defmt heartbeat. Silent without a probe, but
/// useful when one is attached to confirm core 0 is also scheduling.
fn core0_body() -> ! {
    let mut count: u32 = 0;
    loop {
        scars::printkln!(
            "[core0] (core {}) tick {}",
            CoreId::current().as_u8(),
            count
        );
        count = count.wrapping_add(1);
        scars::delay_until(Instant::now() + Duration::from_secs(1));
    }
}

/// Core 1's body: 2 Hz LED toggle (1 Hz visible blink). Exercises
/// the full kernel path on core 1: SVCall → `delay_until` syscall →
/// scheduler block → ALARM_1/TIMER0_IRQ_1 wakeup → resume_thread →
/// PendSV → context switch back. If the LED blinks at 1 Hz after
/// boot, every per-core service-call / IPI / timer / context-switch
/// path on core 1 is healthy.
fn core1_body() -> ! {
    loop {
        toggle_led();
        scars::delay_until(Instant::now() + Duration::from_millis(500));
    }
}

#[scars::init]
#[define_opaque(Core0F, Core1F)]
fn init() {
    init_led();

    // Core-0 thread (same-core start: existing syscall path).
    static STACK0: Stack<STACK_SIZE> = Stack::new();
    static T0: Thread<PRIO, Core0F, { CoreId::DEFAULT }> = Thread::new("core0-ticker");
    let _ = T0.init(STACK0.init()).attach(core0_body).start();

    // Core-1 thread (cross-core start: posts START op into core 1's
    // deferred-work queue and rings its SIO IPI).
    static STACK1: Stack<STACK_SIZE> = Stack::new();
    static T1: Thread<PRIO, Core1F, { CoreId::new(1) }> = Thread::new("core1-blinker");
    let _ = T1.init(STACK1.init()).attach(core1_body).start();
}
