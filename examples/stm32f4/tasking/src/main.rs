//! Demonstrates sending data between two threads over a bounded channel.
//!
//! Tested on STM32F429I-DISC1 board.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
use scars::Stack;
use scars::prelude::*;
use scars::thread::ThreadFn;
use scars::time::{Duration, Instant};

const PRODUCER_PRIORITY: Priority = Priority::thread(3);
const CONSUMER_PRIORITY: Priority = Priority::thread(2);
const CEILING_PRIORITY: Priority = PRODUCER_PRIORITY.max(CONSUMER_PRIORITY);
const THREAD_STACK_SIZE: usize = 1024;
const CHANNEL_CAPACITY: usize = 16;

type ProducerF = impl ThreadFn;
type ConsumerF = impl ThreadFn;

#[scars::init]
#[define_opaque(ProducerF, ConsumerF)]
fn init() {
    let (sender, receiver) = make_channel!(u64, CHANNEL_CAPACITY, CEILING_PRIORITY);

    static PRODUCER_STACK: Stack<THREAD_STACK_SIZE> = Stack::new();
    static PRODUCER_THREAD: Thread<PRODUCER_PRIORITY, ProducerF> = Thread::new("producer");
    let _ = PRODUCER_THREAD
        .init(PRODUCER_STACK.init())
        .attach(move || {
            let mut count: u64 = 0;
            loop {
                scars::printkln!("[producer]: sending {}", count);
                let _ = sender.send(count);
                count += 1;
                scars::delay_until(Instant::now() + Duration::from_secs(1));
            }
        })
        .start();

    static CONSUMER_STACK: Stack<THREAD_STACK_SIZE> = Stack::new();
    static CONSUMER_THREAD: Thread<CONSUMER_PRIORITY, ConsumerF> = Thread::new("consumer");
    let _ = CONSUMER_THREAD
        .init(CONSUMER_STACK.init())
        .attach(move || {
            loop {
                let count = receiver.recv();
                scars::printkln!("[consumer]: received {}", count);
            }
        })
        .start();
}
