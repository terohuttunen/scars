#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::rendezvous::CeilingRendezvous;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const CALLER_PRIORITY: Priority = Priority::thread(5);
const ACCEPTOR_PRIORITY: Priority = Priority::thread(3);
const CEILING: Priority = CALLER_PRIORITY;

type CallerF = impl ThreadFn;
type AcceptorF = impl ThreadFn;

static CALLER_STACK: Stack<STACK_SIZE> = Stack::new();
static CALLER: Thread<CALLER_PRIORITY, CallerF> = Thread::new("caller");
static ACCEPTOR_STACK: Stack<STACK_SIZE> = Stack::new();
static ACCEPTOR: Thread<ACCEPTOR_PRIORITY, AcceptorF> = Thread::new("acceptor");

// Explicit `u32 -> u32` rendezvous so the call result can be checked by the
// caller
static mut RV: CeilingRendezvous<u32, u32, CEILING> = CeilingRendezvous::new();

/// rendezvous call/accept
///
/// CALLER (high priority) issues `entry(21)` and blocks; ACCEPTOR (low
/// priority) services the call, doubling the argument; CALLER unblocks with
/// the result and checks it is 42.
#[scars::init]
#[define_opaque(CallerF, AcceptorF)]
fn init() {
    let (entry, accept) = unsafe { (&mut *core::ptr::addr_of_mut!(RV)).split() };

    CALLER
        .init(CALLER_STACK.init())
        .attach(move || {
            let result = entry.entry(21);
            assert_eq!(result, 42);
            scars_test::test_succeed()
        })
        .start();

    ACCEPTOR
        .init(ACCEPTOR_STACK.init())
        .attach(move || {
            accept.accept(|arg| arg * 2);
            loop {
                scars::delay(Duration::from_secs(60));
            }
        })
        .start();
}
