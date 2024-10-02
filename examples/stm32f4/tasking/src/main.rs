//! This example demonstrates how to send data between two threads using a channel.
//!
//! Tested on STM32F429I-DISC1 board.
#![no_std]
#![no_main]
#![feature(panic_info_message)]
#![feature(type_alias_impl_trait)]
use scars::khal::{Interrupt, Peripherals};
use scars::{delay_until, make_channel, make_thread, AnyPriority, Priority};

const PRODUCER_PRIORITY: u8 = 2;
const CONSUMER_PRIORITY: u8 = 3;
const THREAD_STACK_SIZE: usize = 1024;
const CEILING_PRIORITY: AnyPriority = 3;
const CHANNEL_CAPACITY: usize = 16;

#[scars::entry(name = "main", priority = 1, stack_size = 4096)]
fn main() {
    let producer_thread = make_thread!("producer", PRODUCER_PRIORITY, THREAD_STACK_SIZE);
    let consumer_thread = make_thread!("consumer", CONSUMER_PRIORITY, THREAD_STACK_SIZE);

    let (sender, receiver) = make_channel!(u64, CHANNEL_CAPACITY, CEILING_PRIORITY);

    let mut count = 0;
    producer_thread.start(move || loop {
        scars::printkln!("[producer]: sending {}", count);
        let _ = sender.send(count);
        count += 1;
        delay_until(scars::time::Instant::now() + scars::time::Duration::from_secs(1));
    });

    consumer_thread.start(move || loop {
        let count = receiver.recv();
        scars::printkln!("[consumer]: received {}", count);
    });

    loop {
        scars::idle();
    }
}
