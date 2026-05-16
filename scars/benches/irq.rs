#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

extern crate std;

use core::sync::atomic::{AtomicU64, Ordering};
use scars::Stack;
use scars::interrupt::{InterruptHandler, InterruptHandlerFn};
use scars::khal::pac::Interrupt;
use scars::khal::pend_interrupt;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;

const STACK_SIZE: usize = 16384;

const HANDLER_PRIORITY: Priority = Priority::interrupt(3);
const DRIVER_PRIORITY: Priority = Priority::thread(5);
const CHANNEL_CEILING: Priority = HANDLER_PRIORITY;

const ITERS: u32 = 500;
const PEND_INTERVAL: Duration = Duration::from_millis(2);

static ENTRY_TICK: AtomicU64 = AtomicU64::new(0);

type HandlerF = impl InterruptHandlerFn;
type DriverF = impl ThreadFn;

static HANDLER: InterruptHandler<HANDLER_PRIORITY, HandlerF> = InterruptHandler::new();
static DRIVER_STACK: Stack<STACK_SIZE> = Stack::new();
static DRIVER: Thread<DRIVER_PRIORITY, DriverF> = Thread::new("bench_irq");

#[scars::init]
#[define_opaque(HandlerF, DriverF)]
fn init() {
    let (handler_done_tx, handler_done_rx) = make_channel!(u8, 1, CHANNEL_CEILING);

    HANDLER
        .init(Interrupt::IRQ0 as u16)
        .attach(move || {
            ENTRY_TICK.store(scars::clock_ticks(), Ordering::Release);
            let _ = handler_done_tx.try_send(0);
        })
        .enable();

    DRIVER
        .init(DRIVER_STACK.init())
        .attach(move || {
            let mut m = scars_bench::Measurement::new();
            for _ in 0..ITERS {
                // Space pend attempts so the previous one is fully drained
                // and the host-side simulator interrupt path is quiescent.
                scars::delay(PEND_INTERVAL);
                ENTRY_TICK.store(0, Ordering::Relaxed);
                let pend_ticks = scars::clock_ticks();
                pend_interrupt(Interrupt::IRQ0 as u16);
                let _ = handler_done_rx.recv();
                let entry_ticks = ENTRY_TICK.load(Ordering::Acquire);
                let delta = entry_ticks.saturating_sub(pend_ticks);
                m.add_ns(Duration::from_ticks(delta).as_nanos());
            }
            scars_bench::report("irq_latency", &m);
            scars_bench::bench_done();
        })
        .start();
}
