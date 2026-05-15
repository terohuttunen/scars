//! This example demonstrates how to use EXTI0 interrupt to trigger an event on button press.
//!
//! Tested on STM32F429I-DISC1 board.
#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]
use scars::Stack;
use scars::events::Events;
use scars::khal::{Interrupt, Peripherals, pac::EXTI};
use scars::sync::channel::CeilingSender;
use scars::task::{
    self, EventHandlerExecutor, LocalExecutor, Sleep, WaitForEvents, task_pool::TaskPool,
};
use scars::thread::{Thread, ThreadFn};
use scars::{
    Priority,
    interrupt::{InterruptHandler, InterruptHandlerFn},
    make_channel,
};

const EXTI0_INTERRUPT_PRIO: Priority = Priority::interrupt(1);
const CEILING_PRIO: Priority = EXTI0_INTERRUPT_PRIO;
const CHANNEL_CAPACITY: usize = 16;
const MAIN_PRIORITY: Priority = Priority::thread(1);
const MAIN_STACK_SIZE: usize = 4096;

/// Event sent when button is pressed
const BUTTON_PRESSED: Events = 1 << 0;

type F = impl InterruptHandlerFn;
type G = impl ::core::future::Future;
type MainF = impl ThreadFn;

static EXTI0_POOL: TaskPool<G, 10> = TaskPool::new();

#[scars::init]
#[define_opaque(F, G, MainF)]
fn init() {
    let Peripherals { SYSCFG, EXTI, .. } = Peripherals::take().unwrap();

    // Source EXTI0 interrupt from PA0 GPIO
    SYSCFG.exticr1().write(|w| unsafe { w.exti0().bits(0) });

    // Enable EXTI0 interrupt in EXTI
    EXTI.imr().write(|w| w.mr0().set_bit());

    // Trigger interrupt from rising edge
    EXTI.rtsr().write(|w| w.tr0().set_bit());

    let (sender, receiver) = make_channel!(u32, CHANNEL_CAPACITY, CEILING_PRIO);
    let mut count: u32 = 0;

    static EXTI0_HANDLER: InterruptHandler<EXTI0_INTERRUPT_PRIO, F> = InterruptHandler::new();
    static EXTI0_EXECUTOR: EventHandlerExecutor<EXTI0_INTERRUPT_PRIO> = EventHandlerExecutor::new();

    let executor = EXTI0_EXECUTOR.init().publish().build();
    let task_handle = EXTI0_POOL.alloc().unwrap().attach(|| async move {
        scars::printkln!("EXTI0 async task spawned");
        loop {
            // Wait for button press event from interrupt handler
            WaitForEvents::new(BUTTON_PRESSED).await;
            scars::printkln!("Button pressed!");

            // Count from 1 to 3 with 1 second delay
            for i in 0..3 {
                scars::printkln!("  counting {}/3", i + 1);
                Sleep::sleep(scars::time::Duration::from_millis(1000)).await;
            }

            count += 1;

            let _ = sender.try_send(count);
        }
    });
    // Spawn an async task associated with the EXTI0 interrupt handler
    let _join_handle = executor.spawn(task_handle);

    let exti0 = EXTI0_HANDLER
        .init(Interrupt::EXTI0 as u16)
        .with_shared_storage(&executor)
        .attach(move || {
            scars::printkln!("EXTI0 interrupt received");
            // Clear EXTI0 interrupt flag (write-1-to-clear)
            EXTI.pr().write(|w| w.pr0().clear_bit_by_one());

            // Send event to wake up waiting tasks
            LocalExecutor::get().send_events(BUTTON_PRESSED);
        });

    exti0.enable();

    static MAIN_STACK: Stack<MAIN_STACK_SIZE> = Stack::new();
    static MAIN_THREAD: Thread<MAIN_PRIORITY, MainF> = Thread::new("main");
    let _ = MAIN_THREAD
        .init(MAIN_STACK.init())
        .attach(move || {
            loop {
                let count = receiver.recv();
                scars::printkln!("==> Button event {:?} received", count);
            }
        })
        .start();
}
