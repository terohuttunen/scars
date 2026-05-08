#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use core::pin::Pin;
use scars::EventTimer;
use scars::Events;
use scars::Stack;
use scars::events::handler::{EventHandler, EventHandlerFn};
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::{Duration, Instant};
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024 * 2;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const THREAD_PRIORITY: Priority = Priority::thread(3);
const HANDLER_PRIORITY: Priority = Priority::interrupt(1);
const CHECKER_PRIORITY: Priority = Priority::thread(1);

const CAPACITY: usize = 4;
const CEILING: Priority = HANDLER_PRIORITY;

const TIMER_EVENT: Events = 1u32;
const HANDLER_REPORT: u32 = 0x5A;
const DEADLINE_MS: u64 = 50;

type Thread0F = impl ThreadFn;
type HandlerF = impl EventHandlerFn;
type CheckerF = impl ThreadFn;

static THREAD0_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD0: Thread<THREAD_PRIORITY, Thread0F> = Thread::new("thread0");

static HANDLER0: EventHandler<HANDLER_PRIORITY, HandlerF> = EventHandler::new();
static TIMER0: EventTimer = EventTimer::new();

static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER_THREAD: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

/// An [`EventTimer`] armed to deliver events to an [`EventHandler`] runs
/// the handler closure after its deadline without any thread sending the
/// events directly. The handler closure forwards a sentinel back via a
/// channel; the test asserts the sentinel arrives.
#[scars::init]
#[define_opaque(Thread0F, HandlerF, CheckerF)]
fn init() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    let handler_sender = sender.clone();
    let event_sender = HANDLER0
        .init()
        .attach(move || {
            // try_send: blocking is forbidden in interrupt context.
            let _ = handler_sender.try_send(HANDLER_REPORT);
        })
        .sender();

    THREAD0
        .init(THREAD0_STACK.init())
        .attach(move || {
            let start = Instant::now();
            let deadline = start + Duration::from_millis(DEADLINE_MS);
            let timer: Pin<&'static EventTimer> = Pin::static_ref(&TIMER0);
            timer.arm(deadline, event_sender, TIMER_EVENT);

            // Deadlock guard: the handler should fire well before this elapses.
            scars::delay(Duration::from_millis(DEADLINE_MS * 20));
            scars_test::test_fail()
        })
        .start();

    CHECKER_THREAD
        .init(CHECKER_STACK.init())
        .attach(move || {
            assert_eq!(receiver.recv(), HANDLER_REPORT);
            scars_test::test_succeed()
        })
        .start();
}
