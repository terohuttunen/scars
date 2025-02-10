#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![feature(sync_unsafe_cell)]
use scars;
extern crate std;
use scars::prelude::*;

#[scars::thread(priority = Priority::thread(2), stack_size = 16384)]
fn other() -> ! {
    loop {
        scars::printkln!("Hello, from the other thread!");
        scars::delay(scars::time::Duration::from_millis(10));
    }
}

#[scars::entry(name = "main", priority = 1, stack_size = 16384)]
fn main() -> ! {
    let _ = other().start();

    loop {
        scars::printkln!("Hello, from main!");
        scars::delay(scars::time::Duration::from_millis(10));
    }
}
