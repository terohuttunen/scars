# scars-khal-test

A synchronous test-harness backend for the SCARS kernel HAL (KHAL), alongside the
hardware ports and the pthreads simulator. It models the kernel's hardware view as
plain in-process state so that unit tests run deterministically and can inspect the
modeled hardware directly.

- The clock is a counter the test advances.
- Interrupts are a pending bitmap the test sets and drives into the kernel.
- A context switch is *recorded* — `set_current_thread_context` stores the pointer
  and appends to a switch log — rather than transferring control.

Selected by the `khal-test` feature on the `scars` crate (mutually exclusive with the
other `khal-*` backends). It uses the `display` fault formatter instead of `defmt`,
so the test binary links as an ordinary host executable: no linker script and no
`-no-pie`, unlike the simulator.

## Scope

Because a switch is recorded rather than executed, thread bodies do not run. This
backend is for deterministic kernel-logic and hardware-introspection tests that do
not start threads:

- data-structure and kernel-logic unit tests,
- clock and alarm behaviour,
- the interrupt controller (priority, enable, threshold, pending),
- handlers reached through `pump()`.

Thread lifecycle, preemptive scheduling, and priority inheritance are exercised by
the simulator backend (`scars-khal-sim`), which runs threads for real. A started
thread persists in the global scheduler across `#[test_case]`s, which the
synchronous model cannot unwind.

## Running tests

Unit tests live in `#[cfg(test)]` modules in the `scars` crate and run in idle
context (boot reaches the test runner via `idle()` -> `test_main()`). Run them
through the `test` board:

```bash
cargo xtask test --board test
```

That board sets `lib_only`, so only the crate's lib unit tests run; the
thread-driven integration tests stay with the simulator. The equivalent direct
invocation is:

```bash
cargo test -p scars --lib --features khal-test --target x86_64-unknown-linux-gnu
```

`multithreading` is a default feature, so the kernel is built with threads enabled
even though tests run in idle context.

## Driving and introspection API

| Function | Purpose |
| --- | --- |
| `pend_interrupt(n)` | Set a virtual IRQ pending. |
| `advance(delta)` / `advance_to(ticks)` | Move the clock; fire `_kernel_wakeup_handler` if the armed deadline is crossed. |
| `pump()` | Drive runnable interrupts, then pending service calls, into the kernel until quiescent. |
| `set_manual_pump(bool)` | Suppress the post-syscall/post-wakeup auto-drain so a test can observe the intermediate state and `pump()` itself. |
| `reset()` | Clear peripheral state (clock, alarm, interrupt controller, service-call flag, switch log). Leaves `current_context`, which is coupled to the kernel scheduler. |
| `state() -> HwState` | Snapshot the modeled hardware for assertions. |

`HwState` exposes `now`, `wakeup`, `wakeups_fired`, `threshold`, `pending`,
`interrupts_enabled`, `service_pending`, `current_thread`, `current_context`, the
per-IRQ `irq[]` table, and the ordered `switch_log`.

## Example

```rust
use crate::khal;
use scars_khal::{AlarmClockController, InterruptController};

type Hal = khal::HAL;

#[test_case]
fn armed_wakeup_fires_when_clock_crosses_it() {
    khal::reset();
    <Hal as AlarmClockController>::set_wakeup(Some(1000));

    khal::advance(500);
    assert_eq!(khal::state().wakeups_fired, 0); // deadline not reached

    khal::advance(600); // now 1100 >= 1000
    let s = khal::state();
    assert_eq!(s.wakeups_fired, 1);
    assert!(s.wakeup.is_none()); // disarmed after firing
}
```
