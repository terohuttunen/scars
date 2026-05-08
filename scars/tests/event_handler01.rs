#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Events;
use scars::Stack;
use scars::events::handler::{EventHandler, EventHandlerFn};
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;
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

const TRIGGER_EVENT: Events = 1u32;
const HANDLER_REPORT: u32 = 0xA5;

type Thread0F = impl ThreadFn;
type HandlerF = impl EventHandlerFn;
type CheckerF = impl ThreadFn;

static THREAD0_STACK: Stack<STACK_SIZE> = Stack::new();
static THREAD0: Thread<THREAD_PRIORITY, Thread0F> = Thread::new("thread0");

static HANDLER0: EventHandler<HANDLER_PRIORITY, HandlerF> = EventHandler::new();

static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER_THREAD: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

/// A thread sends an event to a software-interrupt EventHandler.
/// The handler closure runs and forwards a sentinel value back via
/// a channel; the test verifies the value arrives.
#[scars::init]
#[define_opaque(Thread0F, HandlerF, CheckerF)]
fn init() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    let handler_sender = sender.clone();
    let event_sender = HANDLER0
        .init()
        .attach(move || {
            // try_send (not send): blocking is forbidden in interrupt context.
            let _ = handler_sender.try_send(HANDLER_REPORT);
        })
        .sender();

    THREAD0
        .init(THREAD0_STACK.init())
        .attach(move || {
            event_sender.send_events(TRIGGER_EVENT);
            scars::delay(Duration::from_millis(1000));
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
