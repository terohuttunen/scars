#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(sync_unsafe_cell)]
use scars;
extern crate std;
use scars::Stack;
use scars::prelude::*;
use scars::thread::ThreadFn;

type MainF = impl ThreadFn;
type OtherF = impl ThreadFn;

#[scars::init]
#[define_opaque(MainF, OtherF)]
fn init() {
    static MAIN_THREAD_STACK: Stack<16384> = Stack::new();
    static MAIN_THREAD: Thread<{ Priority::thread(1) }, MainF> = Thread::new("main");
    let _ = MAIN_THREAD
        .init(MAIN_THREAD_STACK.init())
        .attach(|| {
            loop {
                scars::printkln!("Hello, from main!");
                scars::delay(scars::time::Duration::from_millis(10));
            }
        })
        .start();

    static OTHER_THREAD_STACK: Stack<16384> = Stack::new();
    static OTHER_THREAD: Thread<{ Priority::thread(2) }, OtherF> = Thread::new("other");
    let _ = OTHER_THREAD
        .init(OTHER_THREAD_STACK.init())
        .attach(|| {
            loop {
                scars::printkln!("Hello, from the other thread!");
                scars::delay(scars::time::Duration::from_millis(10));
            }
        })
        .start();
}
