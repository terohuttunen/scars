#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::CoreCeilingLock;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const HOLDER_PRIORITY: Priority = Priority::thread(3);
const PEER_PRIORITY: Priority = Priority::thread(5);
const PREEMPTOR_PRIORITY: Priority = Priority::thread(10);
const WATCHDOG_PRIORITY: Priority = Priority::thread(2);
const CHECKER_PRIORITY: Priority = Priority::thread(1);
const CEILING: Priority = PEER_PRIORITY;

const HOLDER_ID: u8 = 1;
const PEER_ID: u8 = 2;

type HolderF = impl ThreadFn;
type PeerF = impl ThreadFn;
type PreemptorF = impl ThreadFn;
type WatchdogF = impl ThreadFn;
type CheckerF = impl ThreadFn;

static HOLDER_STACK: Stack<STACK_SIZE> = Stack::new();
static HOLDER: Thread<HOLDER_PRIORITY, HolderF> = Thread::new("holder");
static PEER_STACK: Stack<STACK_SIZE> = Stack::new();
static PEER: Thread<PEER_PRIORITY, PeerF> = Thread::new("peer");
static PREEMPTOR_STACK: Stack<STACK_SIZE> = Stack::new();
static PREEMPTOR: Thread<PREEMPTOR_PRIORITY, PreemptorF> = Thread::new("preemptor");
static WATCHDOG_STACK: Stack<STACK_SIZE> = Stack::new();
static WATCHDOG: Thread<WATCHDOG_PRIORITY, WatchdogF> = Thread::new("watchdog");
static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

/// A ready thread that holds a priority-ceiling lock is dispatched ahead of an
/// equal-active-priority peer that does not.
///
/// HOLDER (base 3) raises to the ceiling (5) in a lock section. Inside it, it
/// makes PEER (base 5) ready — equal active priority, so PEER cannot preempt —
/// then makes a higher-priority preemptor ready. The preemptor preempts HOLDER
/// while it still holds the lock and immediately steps down. HOLDER, queued at
/// the head of priority band 5 because it holds a lock, runs before PEER, so
/// the recorded order is HOLDER, PEER.
#[scars::init]
#[define_opaque(HolderF, PeerF, PreemptorF, WatchdogF, CheckerF)]
fn init() {
    let (tx, rx) = make_channel!(u8, 2, CEILING);

    let holder_tx = tx.clone();
    let peer_tx = tx.clone();
    HOLDER
        .init(HOLDER_STACK.init())
        .attach(move || {
            CoreCeilingLock::<CEILING>::with(|_ckey| {
                let peer_tx = peer_tx.clone();
                PEER.init(PEER_STACK.init())
                    .attach(move || {
                        peer_tx.send(PEER_ID);
                        loop {
                            scars::delay(Duration::from_secs(60));
                        }
                    })
                    .start();

                PREEMPTOR
                    .init(PREEMPTOR_STACK.init())
                    .attach(move || {
                        loop {
                            scars::delay(Duration::from_secs(60));
                        }
                    })
                    .start();

                // Reached after the preemptor steps down; HOLDER is dispatched
                // ahead of PEER because it holds the ceiling lock.
                holder_tx.send(HOLDER_ID);
            });
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();

    WATCHDOG
        .init(WATCHDOG_STACK.init())
        .attach(move || {
            scars::delay(Duration::from_millis(2000));
            scars_test::test_fail()
        })
        .start();

    CHECKER
        .init(CHECKER_STACK.init())
        .attach(move || {
            assert_eq!(rx.recv(), HOLDER_ID);
            assert_eq!(rx.recv(), PEER_ID);
            scars_test::test_succeed()
        })
        .start();
}
