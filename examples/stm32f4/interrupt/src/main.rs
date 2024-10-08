//! This example demonstrates how to use EXTI0 interrupt to trigger an event on button press.
//!
//! Tested on STM32F429I-DISC1 board.
#![no_std]
#![no_main]
#![feature(panic_info_message)]
#![feature(type_alias_impl_trait)]
use scars::khal::{Interrupt, Peripherals};
use scars::task::Sleep;
use scars::{
    kernel::interrupt::wait_for_interrupt, make_channel, make_interrupt_handler, make_task_pool,
    Priority,
};

const EXTI0_INTERRUPT_PRIO: Priority = Priority::interrupt(1);
const CEILING_PRIO: Priority = EXTI0_INTERRUPT_PRIO;

#[scars::entry(name = "main", priority = 1, stack_size = 4096)]
fn main() {
    let Peripherals { SYSCFG, EXTI, .. } = Peripherals::take().unwrap();

    // Source EXTI0 interrupt from PA0 GPIO
    SYSCFG.exticr1.write(|w| unsafe { w.exti0().bits(0) });

    // Enable EXTI0 interrupt in EXTI
    EXTI.imr.write(|w| w.mr0().set_bit());

    // Trigger interrupt from rising edge
    EXTI.rtsr.write(|w| w.tr0().set_bit());

    let (sender, receiver) = make_channel!(u32, 16, CEILING_PRIO);
    let mut count: u32 = 0;

    let mut exti0 =
        make_interrupt_handler!(Interrupt::EXTI0, EXTI0_INTERRUPT_PRIO, 1, executor = true);

    // Spawn an async task associated with the EXTI0 interrupt handler
    let task_pool = make_task_pool!(10);
    let _join_handle = exti0.spawn(&task_pool, async move {
        scars::printkln!("EXTI0 interrupt handler spawned");
        loop {
            wait_for_interrupt().await;

            // Count from 1 to 3 with 1 second delay
            for i in 0..3 {
                scars::printkln!("  counting {}/3", i + 1);
                Sleep::sleep(scars::time::Duration::from_millis(1000)).await;
            }

            count += 1;

            let _ = sender.try_send(count);
        }
    });

    exti0
        .attach(move || {
            scars::printkln!("EXTI0 interrupt received");
            // Clear EXTI0 interrupt flag
            EXTI.pr.write(|w| w.pr0().set_bit());
        })
        .enable(); // Enable EXTI0 interrupt in NVIC

    loop {
        let count = receiver.recv();
        scars::printkln!("==> Button event {:?} received", count);
    }
}
