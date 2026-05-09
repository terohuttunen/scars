//! Nested-interrupt test for scars on Cortex-M.
//!
//! A LOW-priority IRQ handler pends a HIGH-priority IRQ from inside its
//! own closure via `NVIC::pend`. If the kernel's nesting infrastructure
//! is correct, HIGH preempts LOW immediately, runs to completion, and
//! returns control to LOW. The two handlers cooperate on a shared
//! `AtomicU32` to record the sequence of events; the main thread reads
//! and prints the recorded sequence after each round.
//!
//! The IRQs themselves are pended via `NVIC::pend` (no peripheral state
//! involved): EXTI0 plays the role of LOW, EXTI1 the role of HIGH.
//! Their handler closures don't touch EXTI hardware — they just observe
//! the shared sequence.
//!
//! Expected RTT output per round:
//!
//! ```text
//! [main] round N begin
//!   [LOW] start
//!     [HIGH] start (preempting LOW)
//!     [HIGH] end
//!   [LOW] resumed; HIGH preempted as expected
//! [main] round N OK
//! ```
//!
//! A failure surfaces as either a missing `[HIGH]` line or a `[LOW]
//! resumed; HIGH did NOT preempt` message after main finishes.
//!
//! Tested on STM32F429I-DISC1.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::NVIC;
use scars::Stack;
use scars::khal::pac::Interrupt;
use scars::interrupt::{InterruptHandler, InterruptHandlerFn};
use scars::prelude::*;
use scars::thread::{Thread, ThreadFn};
use scars::time::Duration;

const LOW_PRIO: Priority = Priority::interrupt(1);
const HIGH_PRIO: Priority = Priority::interrupt(2);
const MAIN_PRIO: Priority = Priority::thread(1);
const MAIN_STACK_SIZE: usize = 4096;

// Sequence markers shared between the main thread and the two ISRs.
const SEQ_IDLE: u32 = 0;
const SEQ_LOW_ENTER: u32 = 1;
const SEQ_HIGH_ENTER: u32 = 2;
const SEQ_HIGH_EXIT: u32 = 3;
const SEQ_LOW_EXIT: u32 = 4;

static SEQUENCE: AtomicU32 = AtomicU32::new(SEQ_IDLE);

type LowF = impl InterruptHandlerFn;
type HighF = impl InterruptHandlerFn;
type MainF = impl ThreadFn;

#[scars::init]
#[define_opaque(LowF, HighF, MainF)]
fn init() {
    static LOW_HANDLER: InterruptHandler<LOW_PRIO, LowF> = InterruptHandler::new();
    static HIGH_HANDLER: InterruptHandler<HIGH_PRIO, HighF> = InterruptHandler::new();

    let low = LOW_HANDLER
        .init(Interrupt::EXTI0 as u16)
        .attach(|| {
            scars::printkln!("  [LOW] start");
            SEQUENCE.store(SEQ_LOW_ENTER, Ordering::SeqCst);

            // Pend HIGH; because HIGH has higher NVIC priority, the CPU
            // should tail-chain into HIGH immediately and return here
            // only after HIGH completes.
            NVIC::pend(Interrupt::EXTI1);
            // dsb: ensure the volatile write to NVIC ISPR is visible
            //   before we proceed (so the pending bit actually fires).
            // isb: flush the pipeline so the CPU re-evaluates pending
            //   exceptions and tail-chains into HIGH before executing
            //   the next instruction.
            // compiler_fence: prevent LLVM from hoisting the
            //   `SEQUENCE.load` above `NVIC::pend` — Rust's memory
            //   model doesn't order volatile writes against atomics.
            cortex_m::asm::dsb();
            cortex_m::asm::isb();
            core::sync::atomic::compiler_fence(Ordering::SeqCst);

            // After NVIC::pend returns and HIGH has run, SEQUENCE
            // should be SEQ_HIGH_EXIT.
            let observed = SEQUENCE.load(Ordering::SeqCst);
            if observed == SEQ_HIGH_EXIT {
                scars::printkln!("  [LOW] resumed; HIGH preempted as expected");
            } else {
                scars::printkln!(
                    "  [LOW] resumed; HIGH did NOT preempt (sequence={})",
                    observed,
                );
            }
            SEQUENCE.store(SEQ_LOW_EXIT, Ordering::SeqCst);
        });

    let high = HIGH_HANDLER
        .init(Interrupt::EXTI1 as u16)
        .attach(|| {
            let observed = SEQUENCE.load(Ordering::SeqCst);
            if observed == SEQ_LOW_ENTER {
                scars::printkln!("    [HIGH] start (preempting LOW)");
            } else {
                scars::printkln!("    [HIGH] start (UNEXPECTED prev={})", observed);
            }
            SEQUENCE.store(SEQ_HIGH_ENTER, Ordering::SeqCst);
            scars::printkln!("    [HIGH] end");
            SEQUENCE.store(SEQ_HIGH_EXIT, Ordering::SeqCst);
        });

    low.enable();
    high.enable();

    static MAIN_STACK: Stack<MAIN_STACK_SIZE> = Stack::new();
    static MAIN_THREAD: Thread<MAIN_PRIO, MainF> = Thread::new("main");
    let _ = MAIN_THREAD
        .init(MAIN_STACK.init())
        .attach(|| {
            let mut round: u32 = 0;
            loop {
                scars::delay(Duration::from_millis(1000));
                scars::printkln!("[main] round {} begin", round);
                SEQUENCE.store(SEQ_IDLE, Ordering::SeqCst);

                NVIC::pend(Interrupt::EXTI0);

                // Give the IRQs time to fully play out before checking.
                scars::delay(Duration::from_millis(10));

                let final_seq = SEQUENCE.load(Ordering::SeqCst);
                if final_seq == SEQ_LOW_EXIT {
                    scars::printkln!("[main] round {} OK", round);
                } else {
                    scars::printkln!(
                        "[main] round {} FAIL (final sequence={})",
                        round,
                        final_seq,
                    );
                }
                round = round.wrapping_add(1);
            }
        })
        .start();
}
