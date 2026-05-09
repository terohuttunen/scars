//! FPU context save/restore test for Cortex-M4F.
//!
//! Two threads at different priorities each load the callee-saved FPU
//! registers `s16`–`s31` with a thread-specific bit pattern, yield via
//! `scars::delay` (forcing a context switch), then read the registers
//! back and verify they survived. If `_switch_context`'s
//! `vstmiaeq`/`vldmiaeq` save/restore is correct, every iteration prints
//! `[A|B] iter N OK`. Any corruption prints a `FAIL` line naming the
//! offending register and the expected vs actual bit pattern.
//!
//! Tested on STM32F429I-DISC1.
#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]

use core::arch::asm;
use scars::Stack;
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;

const STACK_SIZE: usize = 4096;
const HIGH_PRIORITY: Priority = Priority::thread(2);
const LOW_PRIORITY: Priority = Priority::thread(1);

type HighF = impl ThreadFn;
type LowF = impl ThreadFn;

#[scars::init]
#[define_opaque(HighF, LowF)]
fn init() {
    static HIGH_STACK: Stack<STACK_SIZE> = Stack::new();
    static LOW_STACK: Stack<STACK_SIZE> = Stack::new();
    static HIGH_THREAD: Thread<HIGH_PRIORITY, HighF> = Thread::new("hi");
    static LOW_THREAD: Thread<LOW_PRIORITY, LowF> = Thread::new("lo");

    let _ = HIGH_THREAD
        .init(HIGH_STACK.init())
        .attach(|| run_test('A', 0xA000_0000))
        .start();
    let _ = LOW_THREAD
        .init(LOW_STACK.init())
        .attach(|| run_test('B', 0xB000_0000))
        .start();
}

fn run_test(label: char, base: u32) -> ! {
    let mut iter: u32 = 0;
    loop {
        // Build a 16-element pattern unique per thread. Each register
        // gets a distinct bit pattern, so a wrong restore is detectable
        // by the index it lands at.
        let mut expected: [u32; 16] = [0; 16];
        for i in 0..16 {
            expected[i] = base | iter | (i as u32);
        }

        // Load expected into s16-s31, yield, read back into actual.
        let mut actual: [u32; 16] = [0; 16];
        unsafe {
            asm!(
                ".fpu vfpv4-d16",
                "vldmia {ptr}, {{s16-s31}}",
                ptr = in(reg) expected.as_ptr(),
                out("s16") _, out("s17") _, out("s18") _, out("s19") _,
                out("s20") _, out("s21") _, out("s22") _, out("s23") _,
                out("s24") _, out("s25") _, out("s26") _, out("s27") _,
                out("s28") _, out("s29") _, out("s30") _, out("s31") _,
                options(nostack, preserves_flags),
            );
        }

        // Yield to the other thread; this forces PendSV / context switch.
        scars::delay(Duration::from_millis(5));

        unsafe {
            asm!(
                ".fpu vfpv4-d16",
                "vstmia {ptr}, {{s16-s31}}",
                ptr = in(reg) actual.as_mut_ptr(),
                options(nostack, preserves_flags),
            );
        }

        let mut ok = true;
        for i in 0..16 {
            if actual[i] != expected[i] {
                scars::printkln!(
                    "[{}] FAIL iter={} s{}: expected {:#010x} got {:#010x}",
                    label,
                    iter,
                    16 + i,
                    expected[i],
                    actual[i],
                );
                ok = false;
            }
        }
        if ok {
            scars::printkln!("[{}] iter {} OK", label, iter);
        }

        iter = iter.wrapping_add(1);
    }
}
