# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

SCARS is a Real-Time Operating System (RTOS) designed for hard real-time applications, inspired by the Ada Ravenscar profile. It provides fixed priority preemptive scheduling with a built-in async executor, supporting RISC-V, ARM Cortex-M, and a pthreads-based simulator.

## Build and Test Commands

### Building
```bash
# For RISC-V E310x
cargo build --release --package=scars --features=khal-e310x --target=riscv32imac-unknown-none-elf

# For STM32F4
cargo build --release --package=scars --features=khal-stm32f4 --target=thumbv7em-none-eabihf

# For Simulator
cargo build --release --package=scars --features=khal-sim --target=x86_64-unknown-linux-gnu
```

### Testing
```bash
# Run RISC-V tests (QEMU)
cargo test --release --package=scars --features=khal-e310x --target=riscv32imac-unknown-none-elf

# Run simulator tests
cargo test --release --package=scars --features=khal-sim --target=x86_64-unknown-linux-gnu

# Run STM32F4 tests
cargo test --release --package=scars --features=khal-stm32f4 --target=thumbv7em-none-eabihf

# Run a single test
cargo test --release --package=scars --features=khal-sim --target=x86_64-unknown-linux-gnu test_name
```

### Examples
```bash
# Run simulator examples
cargo run --release --package=sim-examples --bin=example_name

# Run STM32F4 examples
cargo run --release --package=stm32f4-examples --bin=example_name --target=thumbv7em-none-eabihf
```

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

**Event System (`scars/src/events.rs`, `event_set.rs`)**
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

## Development Notes

- Requires nightly Rust (see `rust-toolchain` file)
- Uses `#![no_std]` - avoid std library dependencies
- Stack sizes: 1024 bytes for embedded, 16384 for simulator
- All priority ceiling protocol based synchronization primitives require const generic priority ceiling parameter