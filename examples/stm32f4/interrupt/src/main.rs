//! This example demonstrates how to use EXTI0 interrupt to trigger an event on button press.
//!
//! Tested on STM32F429I-DISC1 board.
#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
extern crate scars;
use scars::khal::{Interrupt, Peripherals, pac::EXTI};
use scars::sync::channel::Sender;
use scars::task::Sleep;
use scars::{Priority, kernel::interrupt::wait_for_interrupt, make_channel};

const EXTI0_INTERRUPT_PRIO: Priority = Priority::interrupt(1);
const CEILING_PRIO: Priority = EXTI0_INTERRUPT_PRIO;
const CHANNEL_CAPACITY: usize = 16;

#[scars::interrupt_handler(interrupt = Interrupt::EXTI0, priority = EXTI0_INTERRUPT_PRIO)]
fn exti0_handler(exti: EXTI) {
    scars::printkln!("EXTI0 interrupt received");
    // Clear EXTI0 interrupt flag
    exti.pr.write(|w| w.pr0().set_bit());
}

#[scars::task(pool_size = 10)]
async fn exti0_async_task(mut count: u32, sender: Sender<u32, CHANNEL_CAPACITY, CEILING_PRIO>) {
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
}

#[scars::entry(name = "main", priority = 1, stack_size = 4096)]
fn main() -> ! {
    let Peripherals { SYSCFG, EXTI, .. } = Peripherals::take().unwrap();

    // Source EXTI0 interrupt from PA0 GPIO
    SYSCFG.exticr1.write(|w| unsafe { w.exti0().bits(0) });

    // Enable EXTI0 interrupt in EXTI
    EXTI.imr.write(|w| w.mr0().set_bit());

    // Trigger interrupt from rising edge
    EXTI.rtsr.write(|w| w.tr0().set_bit());

    let (sender, receiver) = make_channel!(u32, CHANNEL_CAPACITY, CEILING_PRIO);
    let count: u32 = 0;

    // TODO: local storage, executor, task pool
    let exti0 = exti0_handler(EXTI);

    // TODO: how to spawn an async task associated with the EXTI0 interrupt handler
    let _join_handle = exti0.spawn(exti0_async_task(count, sender).unwrap());
    // Spawn an async task associated with the EXTI0 interrupt handler

    exti0.enable();

    loop {
        let count = receiver.recv();
        scars::printkln!("==> Button event {:?} received", count);
    }
}
