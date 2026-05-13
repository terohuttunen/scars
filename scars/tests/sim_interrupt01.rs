#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

extern crate std;

use scars::Stack;
use scars::interrupt::{InterruptHandler, InterruptHandlerFn};
use scars::khal::pac::Interrupt;
use scars::khal::pend_interrupt;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;

const STACK_SIZE: usize = 16384;

const HANDLER_PRIORITY: Priority = Priority::interrupt(3);
const CHECKER_PRIORITY: Priority = Priority::thread(1);
const WATCHDOG_PRIORITY: Priority = Priority::thread(2);

const CAPACITY: usize = 4;
const CEILING: Priority = HANDLER_PRIORITY;

const SENTINEL: u32 = 0xA5;

type HandlerF = impl InterruptHandlerFn;
type CheckerF = impl ThreadFn;
type WatchdogF = impl ThreadFn;

static HANDLER: InterruptHandler<HANDLER_PRIORITY, HandlerF> = InterruptHandler::new();
static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER_THREAD: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");
static WATCHDOG_STACK: Stack<STACK_SIZE> = Stack::new();
static WATCHDOG_THREAD: Thread<WATCHDOG_PRIORITY, WatchdogF> = Thread::new("watchdog");

/// A host pthread pends IRQ0 after 50 ms via the raw
/// [`pend_interrupt`] primitive; the attached handler runs at
/// interrupt priority and forwards a sentinel through a ceiling
/// channel; the checker thread receives it.
#[scars::init]
#[define_opaque(HandlerF, CheckerF, WatchdogF)]
pub fn init() {
    let (sender, receiver) = make_channel!(u32, CAPACITY, CEILING);

    let handler_sender = sender.clone();
    HANDLER
        .init(Interrupt::IRQ0 as u16)
        .attach(move || {
            let _ = handler_sender.try_send(SENTINEL);
        })
        .enable();

    std::thread::Builder::new()
        .name("button".into())
        .spawn(|| {
            std::thread::sleep(core::time::Duration::from_millis(50));
            pend_interrupt(Interrupt::IRQ0 as u16);
        })
        .expect("button thread spawn");

    WATCHDOG_THREAD
        .init(WATCHDOG_STACK.init())
        .attach(move || {
            scars::delay(Duration::from_millis(2000));
            scars_test::test_fail()
        })
        .start();

    CHECKER_THREAD
        .init(CHECKER_STACK.init())
        .attach(move || {
            assert_eq!(receiver.recv(), SENTINEL);
            scars_test::test_succeed()
        })
        .start();
}
