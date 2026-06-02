//! Minimal event-handler-only application. Builds with and without the
//! `threads` feature, so it can be linked both ways to measure the
//! flash-size difference of the multi-threading machinery.
#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]
use scars::Priority;
use scars::events::Events;
use scars::interrupt::{InterruptHandler, InterruptHandlerFn};
use scars::khal::{Interrupt, Peripherals, pac::EXTI};

const EXTI0_INTERRUPT_PRIO: Priority = Priority::interrupt(1);
const BUTTON_PRESSED: Events = 1 << 0;

type F = impl InterruptHandlerFn;

#[scars::init]
#[define_opaque(F)]
fn init() {
    let Peripherals { SYSCFG, EXTI, .. } = Peripherals::take().unwrap();
    SYSCFG.exticr1().write(|w| unsafe { w.exti0().bits(0) });
    EXTI.imr().write(|w| w.mr0().set_bit());
    EXTI.rtsr().write(|w| w.tr0().set_bit());

    static EXTI0_HANDLER: InterruptHandler<EXTI0_INTERRUPT_PRIO, F> = InterruptHandler::new();
    let exti0 = EXTI0_HANDLER.init(Interrupt::EXTI0 as u16).attach(move || {
        EXTI.pr().write(|w| w.pr0().clear_bit_by_one());
        scars::printkln!("button {}", BUTTON_PRESSED);
    });
    exti0.enable();
}
