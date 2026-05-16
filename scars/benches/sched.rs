#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};

// Differentiated stack sizes so the 7-thread sched bench fits in
// the F0 8KB SRAM. DRIVER does printkln of formatted results and
// needs more headroom; helpers run tight yield/send/recv/spin loops
// and use very little stack (Cortex-M syscalls run in handler mode
// on MSP, so kernel work doesn't count against the thread PSP).
#[cfg(not(feature = "khal-sim"))]
const DRIVER_STACK_SIZE: usize = 768;
#[cfg(not(feature = "khal-sim"))]
const HELPER_STACK_SIZE: usize = 512;
#[cfg(feature = "khal-sim")]
const DRIVER_STACK_SIZE: usize = 16384;
#[cfg(feature = "khal-sim")]
const HELPER_STACK_SIZE: usize = 16384;

const DRIVER_PRIORITY: Priority = Priority::thread(7);
const HIGH_PRIORITY: Priority = Priority::thread(6);
const HELPER_PRIORITY: Priority = Priority::thread(5);
const BUSY_PRIORITY: Priority = Priority::thread(3);
const CHANNEL_CEILING: Priority = Priority::thread(7);

// Batched sampling for the fast (sub-microsecond / few-microsecond)
// metrics: each outer sample brackets INNER ops between one pair of
// `Instant::now()` calls, so the per-op cost is amortized over INNER
// ops while still producing SAMPLES data points for min/max/avg.
//
// SAMPLES values are sized so the per-bench avg is statistically
// stable across successive runs on hardware (stderr ∝ 1/√SAMPLES, so
// 1000 samples cuts run-to-run drift ~3× vs 100). INNER stays modest
// — once the clock-read overhead is amortized below a few percent of
// the measured op, adding more inner ops just lengthens wall time
// without improving the per-sample signal.
const CTX_SAMPLES: u32 = 1000;
const CTX_INNER: u32 = 10;
const CHAN_SAMPLES: u32 = 1000;
const CHAN_INNER: u32 = 10;
const PREEMPT_ITERS: u32 = 500;
const PREEMPT_DELAY: Duration = Duration::from_millis(5);
const LONG_DELAY: Duration = Duration::from_secs(60);

type DriverF = impl ThreadFn;
type CtxAF = impl ThreadFn;
type CtxBF = impl ThreadFn;
type ChanAF = impl ThreadFn;
type ChanBF = impl ThreadFn;
type BusyF = impl ThreadFn;
type HighF = impl ThreadFn;

static DRIVER_STACK: Stack<DRIVER_STACK_SIZE> = Stack::new();
static DRIVER: Thread<DRIVER_PRIORITY, DriverF> = Thread::new("bench_sched");

static CTX_A_STACK: Stack<HELPER_STACK_SIZE> = Stack::new();
static CTX_A: Thread<HELPER_PRIORITY, CtxAF> = Thread::new("ctx_a");
static CTX_B_STACK: Stack<HELPER_STACK_SIZE> = Stack::new();
static CTX_B: Thread<HELPER_PRIORITY, CtxBF> = Thread::new("ctx_b");

static CHAN_A_STACK: Stack<HELPER_STACK_SIZE> = Stack::new();
static CHAN_A: Thread<HELPER_PRIORITY, ChanAF> = Thread::new("chan_a");
static CHAN_B_STACK: Stack<HELPER_STACK_SIZE> = Stack::new();
static CHAN_B: Thread<HELPER_PRIORITY, ChanBF> = Thread::new("chan_b");

static BUSY_STACK: Stack<HELPER_STACK_SIZE> = Stack::new();
static BUSY: Thread<BUSY_PRIORITY, BusyF> = Thread::new("preempt_busy");
static HIGH_STACK: Stack<HELPER_STACK_SIZE> = Stack::new();
static HIGH: Thread<HIGH_PRIORITY, HighF> = Thread::new("preempt_high");

#[scars::init]
#[define_opaque(DriverF, CtxAF, CtxBF, ChanAF, ChanBF, BusyF, HighF)]
fn init() {
    let (ctx_a_done_tx, ctx_a_done_rx) =
        make_channel!(scars_bench::Measurement, 1, CHANNEL_CEILING);
    let (ctx_b_done_tx, ctx_b_done_rx) = make_channel!(u8, 1, CHANNEL_CEILING);
    let (chan_a_start_tx, chan_a_start_rx) = make_channel!(u8, 1, CHANNEL_CEILING);
    let (chan_b_start_tx, chan_b_start_rx) = make_channel!(u8, 1, CHANNEL_CEILING);
    let (chan_a_done_tx, chan_a_done_rx) =
        make_channel!(scars_bench::Measurement, 1, CHANNEL_CEILING);
    let (chan_b_done_tx, chan_b_done_rx) = make_channel!(u8, 1, CHANNEL_CEILING);
    let (ping_tx, ping_rx) = make_channel!(u32, 1, CHANNEL_CEILING);
    let (pong_tx, pong_rx) = make_channel!(u32, 1, CHANNEL_CEILING);
    let (busy_start_tx, busy_start_rx) = make_channel!(u8, 1, CHANNEL_CEILING);
    let (high_start_tx, high_start_rx) = make_channel!(u8, 1, CHANNEL_CEILING);
    let (preempt_done_tx, preempt_done_rx) =
        make_channel!(scars_bench::Measurement, 1, CHANNEL_CEILING);

    CTX_A
        .init(CTX_A_STACK.init())
        .attach(move || {
            let mut m = scars_bench::Measurement::new();
            for _ in 0..CTX_SAMPLES {
                let start = Instant::now();
                for _ in 0..CTX_INNER {
                    scars::thread_yield();
                }
                let elapsed_ns = (Instant::now() - start).as_nanos();
                // Each A yield = one A→B switch; B's interleaved
                // yields add ~CTX_INNER B→A switches, so the sample
                // covers ≈2*CTX_INNER context switches.
                m.add_ns(elapsed_ns / (2 * CTX_INNER as u64));
            }
            ctx_a_done_tx.send(m);
            loop {
                scars::delay(LONG_DELAY);
            }
        })
        .start();

    CTX_B
        .init(CTX_B_STACK.init())
        .attach(move || {
            for _ in 0..(CTX_SAMPLES * CTX_INNER) {
                scars::thread_yield();
            }
            ctx_b_done_tx.send(0);
            loop {
                scars::delay(LONG_DELAY);
            }
        })
        .start();

    CHAN_A
        .init(CHAN_A_STACK.init())
        .attach(move || {
            let _ = chan_a_start_rx.recv();
            let mut m = scars_bench::Measurement::new();
            let mut counter: u32 = 0;
            for _ in 0..CHAN_SAMPLES {
                let start = Instant::now();
                for _ in 0..CHAN_INNER {
                    ping_tx.send(counter);
                    let _ = pong_rx.recv();
                    counter = counter.wrapping_add(1);
                }
                let elapsed_ns = (Instant::now() - start).as_nanos();
                m.add_ns(elapsed_ns / CHAN_INNER as u64);
            }
            chan_a_done_tx.send(m);
            loop {
                scars::delay(LONG_DELAY);
            }
        })
        .start();

    CHAN_B
        .init(CHAN_B_STACK.init())
        .attach(move || {
            let _ = chan_b_start_rx.recv();
            for _ in 0..(CHAN_SAMPLES * CHAN_INNER) {
                let v = ping_rx.recv();
                pong_tx.send(v);
            }
            chan_b_done_tx.send(0);
            loop {
                scars::delay(LONG_DELAY);
            }
        })
        .start();

    BUSY.init(BUSY_STACK.init())
        .attach(move || {
            let _ = busy_start_rx.recv();
            loop {
                core::hint::spin_loop();
            }
        })
        .start();

    HIGH.init(HIGH_STACK.init())
        .attach(move || {
            let _ = high_start_rx.recv();
            let mut m = scars_bench::Measurement::new();
            for _ in 0..PREEMPT_ITERS {
                let target = Instant::now() + PREEMPT_DELAY;
                scars::delay_until(target);
                let actual = Instant::now();
                m.add_ns((actual - target).as_nanos());
            }
            preempt_done_tx.send(m);
            loop {
                scars::delay(LONG_DELAY);
            }
        })
        .start();

    DRIVER
        .init(DRIVER_STACK.init())
        .attach(move || {
            let m_cs = ctx_a_done_rx.recv();
            let _ = ctx_b_done_rx.recv();
            scars_bench::report("context_switch", &m_cs);

            chan_a_start_tx.send(0);
            chan_b_start_tx.send(0);

            let m_rt = chan_a_done_rx.recv();
            let _ = chan_b_done_rx.recv();
            scars_bench::report("channel_round_trip", &m_rt);

            busy_start_tx.send(0);
            high_start_tx.send(0);
            let m_pre = preempt_done_rx.recv();
            scars_bench::report("preemption_latency", &m_pre);

            scars_bench::bench_done();
        })
        .start();
}
