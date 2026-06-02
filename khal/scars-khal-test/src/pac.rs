//! Virtual interrupt enumeration for the test harness. Mirrors the
//! simulator: `IRQ0..=IRQ31` are synthetic slots that tests pend with
//! [`crate::pend_interrupt`] and drive into the kernel with
//! [`crate::pump`].

/// Virtual interrupt enumeration for the test harness.
#[repr(u16)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Interrupt {
    IRQ0 = 0,
    IRQ1,
    IRQ2,
    IRQ3,
    IRQ4,
    IRQ5,
    IRQ6,
    IRQ7,
    IRQ8,
    IRQ9,
    IRQ10,
    IRQ11,
    IRQ12,
    IRQ13,
    IRQ14,
    IRQ15,
    IRQ16,
    IRQ17,
    IRQ18,
    IRQ19,
    IRQ20,
    IRQ21,
    IRQ22,
    IRQ23,
    IRQ24,
    IRQ25,
    IRQ26,
    IRQ27,
    IRQ28,
    IRQ29,
    IRQ30,
    IRQ31,
}
