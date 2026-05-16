#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 768;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const DRIVER_PRIORITY: Priority = Priority::thread(5);

const ITERS: u32 = 500;
const DELAY_PER_ITER: Duration = Duration::from_millis(5);

type DriverF = impl ThreadFn;

static DRIVER_STACK: Stack<STACK_SIZE> = Stack::new();
static DRIVER: Thread<DRIVER_PRIORITY, DriverF> = Thread::new("bench_delay");

#[scars::init]
#[define_opaque(DriverF)]
fn init() {
    DRIVER
        .init(DRIVER_STACK.init())
        .attach(move || {
            let mut m = scars_bench::Measurement::new();
            for _ in 0..ITERS {
                let target = Instant::now() + DELAY_PER_ITER;
                scars::delay_until(target);
                let actual = Instant::now();
                m.add_ns((actual - target).as_nanos());
            }
            scars_bench::report("delay_jitter", &m);
            scars_bench::bench_done();
        })
        .start();
}
