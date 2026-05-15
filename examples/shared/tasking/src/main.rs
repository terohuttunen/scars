//! Minimal tasking example. Two threads at different priorities print
//! a message and yield via `scars::delay`.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
use scars::Stack;
use scars::prelude::*;
use scars::thread::ThreadFn;

type LowF = impl ThreadFn;
type HighF = impl ThreadFn;

#[scars::init]
#[define_opaque(LowF, HighF)]
fn init() {
    static LOW_STACK: Stack<1024> = Stack::new();
    static LOW_THREAD: Thread<{ Priority::thread(1) }, LowF> = Thread::new("low");
    let _ = LOW_THREAD
        .init(LOW_STACK.init())
        .attach(|| loop {
            scars::printkln!("[low]: tick");
            scars::delay(scars::time::Duration::from_millis(1000));
        })
        .start();

    static HIGH_STACK: Stack<1024> = Stack::new();
    static HIGH_THREAD: Thread<{ Priority::thread(2) }, HighF> = Thread::new("high");
    let _ = HIGH_THREAD
        .init(HIGH_STACK.init())
        .attach(|| loop {
            scars::printkln!("[high]: tick");
            scars::delay(scars::time::Duration::from_millis(500));
        })
        .start();
}
