//! This example demonstrates how to send data between two threads using a channel.
//!
//! Tested on STM32F429I-DISC1 board.
#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
use scars::prelude::*;
use scars::sync::channel::{Receiver, Sender};
use scars::time::{Duration, Instant};

const PRODUCER_PRIORITY: Priority = Priority::thread(3);
const CONSUMER_PRIORITY: Priority = Priority::thread(2);
const CEILING_PRIORITY: Priority = PRODUCER_PRIORITY.max(CONSUMER_PRIORITY);
const THREAD_STACK_SIZE: usize = 1024;
const CHANNEL_CAPACITY: usize = 16;

#[scars::thread(name = "producer", priority = PRODUCER_PRIORITY, stack_size = THREAD_STACK_SIZE)]
fn producer_thread(mut count: u64, sender: Sender<u64, CHANNEL_CAPACITY, CEILING_PRIORITY>) -> ! {
    loop {
        scars::printkln!("[producer]: sending {}", count);
        let _ = sender.send(count);
        count += 1;
        scars::delay_until(Instant::now() + Duration::from_secs(1));
    }
}

#[scars::thread(name = "consumer", priority = CONSUMER_PRIORITY, stack_size = THREAD_STACK_SIZE)]
fn consumer_thread(receiver: Receiver<u64, CHANNEL_CAPACITY, CEILING_PRIORITY>) -> ! {
    loop {
        let count = receiver.recv();
        scars::printkln!("[consumer]: received {}", count);
    }
}

#[scars::entry(stack_size = 4096)]
fn main() -> ! {
    let (sender, receiver) = make_channel!(u64, CHANNEL_CAPACITY, CEILING_PRIORITY);

    let count: u64 = 0;
    producer_thread(count, sender).start();

    consumer_thread(receiver).start();

    loop {
        scars::idle();
    }
}
