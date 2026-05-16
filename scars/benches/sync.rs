#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::CeilingMutex;
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};

#[cfg(not(feature = "khal-sim"))]
const DRIVER_STACK_SIZE: usize = 768;
#[cfg(not(feature = "khal-sim"))]
const HELPER_STACK_SIZE: usize = 512;
#[cfg(feature = "khal-sim")]
const DRIVER_STACK_SIZE: usize = 16384;
#[cfg(feature = "khal-sim")]
const HELPER_STACK_SIZE: usize = 16384;

const DRIVER_PRIORITY: Priority = Priority::thread(6);
const WAITER_PRIORITY: Priority = Priority::thread(5);
const HOLDER_PRIORITY: Priority = Priority::thread(3);

// Two mutexes. UNCONTENDED's ceiling matches the driver so the
// driver can lock it from its own context. CONTENDED's ceiling
// matches WAITER so the low-prio HOLDER boosts to WAITER's prio
// while held — which is what blocks WAITER's wake-up below.
const UNCONTENDED_CEILING: Priority = DRIVER_PRIORITY;
const CONTENDED_CEILING: Priority = WAITER_PRIORITY;
const CHANNEL_CEILING: Priority = DRIVER_PRIORITY;

// Batched sampling: each outer sample times INNER lock/unlock pairs
// between one pair of Instant::now() calls. The clock-read cost is
// amortized over INNER ops while we still get SAMPLES data points
// for min/max/avg. SAMPLES is sized so the avg is stable across
// successive runs on hardware (stderr ∝ 1/√SAMPLES).
const UNCONTENDED_SAMPLES: u32 = 1000;
const UNCONTENDED_INNER: u32 = 100;

const CONTENDED_ITERS: u32 = 500;
const HOLD_DURATION: Duration = Duration::from_micros(500);
const GAP_DURATION: Duration = Duration::from_micros(500);
const WAITER_PERIOD: Duration = Duration::from_micros(700);

const LONG_DELAY: Duration = Duration::from_secs(60);

type DriverF = impl ThreadFn;
type HolderF = impl ThreadFn;
type WaiterF = impl ThreadFn;

static DRIVER_STACK: Stack<DRIVER_STACK_SIZE> = Stack::new();
static DRIVER: Thread<DRIVER_PRIORITY, DriverF> = Thread::new("bench_sync");
static HOLDER_STACK: Stack<HELPER_STACK_SIZE> = Stack::new();
static HOLDER: Thread<HOLDER_PRIORITY, HolderF> = Thread::new("holder");
static WAITER_STACK: Stack<HELPER_STACK_SIZE> = Stack::new();
static WAITER: Thread<WAITER_PRIORITY, WaiterF> = Thread::new("waiter");

static UNCONTENDED_MUTEX: CeilingMutex<u32, UNCONTENDED_CEILING> = CeilingMutex::new(0);
static CONTENDED_MUTEX: CeilingMutex<u32, CONTENDED_CEILING> = CeilingMutex::new(0);

#[scars::init]
#[define_opaque(DriverF, HolderF, WaiterF)]
fn init() {
    let (holder_start_tx, holder_start_rx) = make_channel!(u8, 1, CHANNEL_CEILING);
    let (waiter_start_tx, waiter_start_rx) = make_channel!(u8, 1, CHANNEL_CEILING);
    let (contended_done_tx, contended_done_rx) =
        make_channel!(scars_bench::Measurement, 1, CHANNEL_CEILING);

    // HOLDER takes the ceiling-protected mutex, occupies CPU at the
    // boosted ceiling priority for HOLD_DURATION, releases, briefly
    // yields the CPU, then repeats. Runs until the process exits at
    // bench_done.
    HOLDER
        .init(HOLDER_STACK.init())
        .attach(move || {
            let _ = holder_start_rx.recv();
            loop {
                let guard = CONTENDED_MUTEX.lock();
                let hold_until = Instant::now() + HOLD_DURATION;
                while Instant::now() < hold_until {
                    core::hint::spin_loop();
                }
                drop(guard);
                scars::delay(GAP_DURATION);
            }
        })
        .start();

    // WAITER sleeps for WAITER_PERIOD between wakes and records the
    // wake-up jitter. Under Immediate Ceiling Protocol, WAITER (prio
    // == CONTENDED_CEILING) cannot preempt HOLDER while HOLDER holds
    // the mutex (boosted to the same priority). Wakes that land
    // during HOLDER's hold window block until HOLDER releases —
    // that's the IPCP analog of "contended" mutex wait time.
    WAITER
        .init(WAITER_STACK.init())
        .attach(move || {
            let _ = waiter_start_rx.recv();
            let mut m = scars_bench::Measurement::new();
            for _ in 0..CONTENDED_ITERS {
                let target = Instant::now() + WAITER_PERIOD;
                scars::delay_until(target);
                let actual = Instant::now();
                m.add_ns((actual - target).as_nanos());
            }
            contended_done_tx.send(m);
            loop {
                scars::delay(LONG_DELAY);
            }
        })
        .start();

    DRIVER
        .init(DRIVER_STACK.init())
        .attach(move || {
            let mut m_u = scars_bench::Measurement::new();
            for _ in 0..UNCONTENDED_SAMPLES {
                let start = Instant::now();
                for _ in 0..UNCONTENDED_INNER {
                    let g = UNCONTENDED_MUTEX.lock();
                    core::hint::black_box(&g);
                    drop(g);
                }
                let elapsed_ns = (Instant::now() - start).as_nanos();
                m_u.add_ns(elapsed_ns / UNCONTENDED_INNER as u64);
            }
            scars_bench::report("mutex_uncontended", &m_u);

            // Despite the conventional name, "mutex contention" in IPCP
            // is not a lock-wait queue — the kernel never lets two
            // threads attempt the same mutex. What it does measure is
            // ceiling-induced wakeup blocking: WAITER's alarm fires
            // while HOLDER is in its boosted critical section, and
            // WAITER cannot start running until HOLDER releases. The
            // reported number is per-wake jitter (actual - target).
            holder_start_tx.send(0);
            waiter_start_tx.send(0);
            let m_c = contended_done_rx.recv();
            scars_bench::report("mutex_ceiling_block", &m_c);

            scars_bench::bench_done();
        })
        .start();
}
