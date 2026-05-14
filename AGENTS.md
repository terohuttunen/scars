# AGENTS.md

## Overview

SCARS is a Real-Time Operating System (RTOS) designed for hard real-time applications,
inspired by the Ada Ravenscar profile. It provides fixed priority preemptive scheduling
with a built-in async executor, supporting RISC-V, ARM Cortex-M, and a pthreads-based simulator.

## Build and Test Commands

All build/test/run/flash commands go through the workspace `xtask` crate.
Boards are declared as TOML manifests under `boards/`, each binding a target
triple, scars features, linker script, env, and runner. `xtask` projects
those into per-invocation cargo environment variables, so a single triple
can host multiple boards.

```bash
# List configured boards
cargo xtask boards

# Check / build a single board (or `all`)
cargo xtask check --board sim
cargo xtask build --board e310x-qemu
cargo xtask build --board all

# Run integration tests (probe-rs boards skipped under `all` unless --include-hw)
cargo xtask test --board sim
cargo xtask test --board e310x-qemu
cargo xtask test --board sim some_filter

# Run / flash an example
cargo xtask run   --board stm32f429i-disco --example interrupt
cargo xtask flash --board stm32f429i-disco --example interrupt
```

Direct `cargo build/test --target=…` at the repo root is **not** supported:
the linker script and runner now live in board manifests, not
`.cargo/config.toml`. Always go through `cargo xtask`.

### Adding a board

1. Drop a `boards/<name>.toml` next to the existing ones; mirror the closest
   sibling.
2. If the board needs a new KHAL crate, add it under `khal/` and wire the
   `khal-<name>` feature in `scars/Cargo.toml`.
3. The board's manifest `name` must match its filename.
4. Each khal crate selects its own `portable-atomic` backend by declaring
   `portable-atomic` in its `Cargo.toml` with the features the target
   requires (e.g. `critical-section` for single-core targets without native
   64-bit atomics). `scars` keeps a bare dep; Cargo unifies the khal's
   choice into the kernel.

### Adding an example

Examples live under `examples/<board-family>/<slug>/`. The crate's
`package.name` **must equal the directory name** — `xtask run --board X
--example <slug>` resolves the slug to that crate. To avoid stale state, also
ensure `examples_dir` in the relevant `boards/<name>.toml` points at the right
family directory.

## Architecture

### Core Components

**Kernel (`scars/src/kernel/`)**
- `scheduler.rs`: Fixed priority preemptive scheduler implementation
- `idle.rs`: Idle thread implementation

**Synchronization (`scars/src/sync/`)**
- `mutex.rs`: Mutex with immediate priority ceiling protocol
- `channel.rs`: Bounded channels for inter-thread communication
- `condition_variable.rs`: Condition variables with priority ceiling
- `atomic_queue.rs`: Lock-free atomic queue implementation

**Threading (`scars/src/thread/`)**
- Thread lifecycle management with static allocation
- Priority inheritance and ceiling protocols
- Stack management and context switching

**Time Management (`scars/src/time.rs`)**
- Monotonic time tracking
- Delay and timeout functionality
- Alarm clock integration

**Event System (`scars/src/events.rs`)**
- Event flags for synchronization
- Atomic event set operations

### Hardware Abstraction Layer (KHAL)

The system uses trait-based HAL with the following key traits in `scars-khal/src/`:
- `InterruptController`: Manages interrupts and priority thresholds
- `AlarmClockController`: Provides monotonic timing and wakeup
- `FlowController`: Handles thread context switching
- `ContextInfo`: Thread context management

Platform implementations:
- `khal/scars-khal-e310x/`: RISC-V E310x
- `khal/scars-khal-stm32f4/`: STM32F4
- `khal/scars-khal-sim/`: Pthreads-based simulator

### Macros and Attributes

**Thread Definition**:
```rust
#[scars_macros::thread(name = "MyThread", stack_size = 1024, priority = 5)]
fn my_thread() {
    // Thread code
}
```

**Interrupt Handler**:
```rust
#[scars_macros::interrupt(PendSV, priority = 1)]
fn pendsv_handler() {
    // Handler code
}
```

### Testing Framework

SCARS uses a custom test framework (`scars-test`) designed for embedded environments:
- Supports multiple output backends (semihosting, RTT, std)
- Integration test macro for easy test setup
- Hardware-in-the-loop testing support

#### Writing Unit Tests

**Test Module Structure**:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::Ordering;

    #[test_case]
    fn test_example() {
        // Test implementation
        assert_eq!(value, expected);
    }
}
```

**Key Differences from Standard Rust Tests**:
- Use `#[test_case]` instead of `#[test]`
- Place tests in `#[cfg(test)]` modules within source files
- Available assertions: `assert!()`, `assert_eq!()`, `assert_ne!()`
- No `std` library - use `core` for basic functionality
- No `format!()` macro - avoid string formatting in tests

**Running Unit Tests**:
```bash
# Run all unit tests in a package
cargo test --package=scars --lib --features=khal-sim --target=x86_64-unknown-linux-gnu

# Run specific test module
cargo test --package=scars --lib module_name::tests --features=khal-sim --target=x86_64-unknown-linux-gnu
```

**Example Unit Test**:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_constructor() {
        let item = MyStruct::new();
        assert_eq!(item.value(), 0);
        assert!(!item.is_active());
    }

    #[test_case]
    fn test_atomic_operations() {
        let atomic_val = AtomicU32::new(0);
        let prev = atomic_val.fetch_or(0x01, Ordering::SeqCst);
        assert_eq!(prev, 0);
        assert_eq!(atomic_val.load(Ordering::SeqCst), 0x01);
    }
}
```

## Development Notes

- Requires nightly Rust (see `rust-toolchain` file)
- Uses `#![no_std]` - avoid std library dependencies
- Stack sizes: 1024 bytes for embedded, 16384 for simulator
- All priority ceiling protocol based synchronization primitives require const generic priority ceiling parameter
- Avoid monomorphization of generic functions. Generic code should be used for ergonomics in the
  interface, but the underlying implementation should not generate new code for each instantiation.
  
  
