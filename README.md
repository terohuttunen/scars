# SCARS
SCARS is a Real-Time Operating System (RTOS) designed for hard real-time applications,
inspired by the Ada Ravenscar profile. It is a traditional style RTOS with fixed
priority preemptive scheduling, and built-in async executor. **It is still a work-in-progress,
and is not recommended for production systems.** Currently only RISC-V and ARM Cortex-M without FPU,
as well as pthreads based simulator are implemented. FPU support is planned.

  - It implements the immediate priority ceiling protocol, and provides an unified
    priority model for threads and interrupts, which allows developers to use the same
    APIs in threads and interrupts. Mutex protected data can be accessed from interrupt
    handlers.

  - All threads and interrupt handlers are statically allocated, yet started at runtime,
    allowing threads and interrupt handlers to capture variables from the environment that
    they are started in. No need for global static variables.

  - All scheduling operations that depend on the number or order of threads, are
    executed within a preemption lock that allows interrupts, minimizing the
    maximum interrupt latency. Interrupt latency is independent from scheduling
    and number of threads.

  - Any priority based synchronization primitives that are below a priority ceiling,
    will not prevent higher priority threads or interrupts from executing.

  - Built-in async executor allows associating async tasks to interrupt handlers.
    Executor is polled on-interrupt, and by the kernel when needed.

## Threads
The entry thread is defined with `entry` attribute for a function. There can be only
one entry thread. Threads can never exit.
```rust
#[scars::entry(name = "main", priority = 1, stack_size = 2048)]
fn main() {
    loop {}
}
```
Other threads are defined with a the `thread` attribute macro, which statically
allocates the thread state, including the thread stack. The thread is created by calling
the thread function, and started with the `start` method.
```rust
const THREAD_PRIO: Priority = Priority::thread(2);
const THREAD_STACK_SIZE: usize = 1024;
const CEILING_PRIO: Priority = THREAD_PRIO;
const CHANNEL_CAPACITY: usize = 1;

#[scars::thread(name = "thread", priority = THREAD_PRIO, stack_size = THREAD_STACK_SIZE)]
fn thread(sender: Sender<u32, CEILING_PRIO>) {
    let mut counter: u32 = 0;
    loop {
        sender.send(counter);
        counter += 1;
    }
}

#[scars::entry(name = "main", priority = 1, stack_size = 2048)]
fn main() {
    let (sender, receiver) = make_channel!(u32, CHANNEL_CAPACITY, CEILING_PRIO);

    thread(sender).start();

    loop {
        let value = receiver.recv();
        println!("Received {:?}", value);
    }
}
```

### Idle thread
The kernel creates an internal idle thread to be run when no other thread is ready.
The idle thread is special in that it does not follow the priority ceiling protocol,
and can run even when higher priority threads hold locks.

To run custom code in the idle thread, the user can define the idle thread hook with
the `idle_thread_hook` attribute:
```rust
#[scars::idle_thread_hook]
fn idle_thread_hook() {
    // do something when idle
}
```
The hook-function can use `InterruptLock` and `PreemptLock`; however, due to being exempt
from the priority ceiling protocol, it may not acquire a `CeilingLock`. An attempt to
acquire a `CeilingLock` from the idle thread will result in a runtime error.

## Interrupt Handlers
Interrupt handlers are created similarly to threads with `interrupt_handler` attribute
macro.
```rust
use scars::khal::{Interrupt, Peripherals, pac::EXTI};
use scars::sync::channel::Sender;
const INTERRUPT_PRIO: Priority = Priority::interrupt(2);
const CEILING_PRIO: Priority = INTERRUPT_PRIO;
const CHANNEL_CAPACITY: usize = 10;

#[scars::interrupt_handler(interrupt = Interrupt::EXTI0, priority = INTERRUPT_PRIO)]
fn exti0_handler(sender: Sender<u32, CEILING_PRIO>, mut counter: u32, exti: EXTI) {
    // Interrupt handler closure captures `sender` and `counter`
    counter += 1;
    // Must use non-blocking `try_send` in interrupt
    let _ = sender.try_send(counter);

    // Clear EXTI0 interrupt flag
    exti.pr.write(|w| w.pr0().set_bit());
}

#[scars::entry(name = "main", priority = 1, stack_size = 2048)]
fn main() {
    let Peripherals { SYSCFG, EXTI, .. } = Peripherals::take().unwrap();

    // Source EXTI0 interrupt from PA0 GPIO
    SYSCFG.exticr1.write(|w| unsafe { w.exti0().bits(0) });

    // Enable EXTI0 interrupt in EXTI
    EXTI.imr.write(|w| w.mr0().set_bit());

    // Trigger interrupt from rising edge
    EXTI.rtsr.write(|w| w.tr0().set_bit());

    let (sender, receiver) = make_channel!(u32, CHANNEL_CAPACITY, CEILING_PRIO);
    let counter: u32 = 0;

    let exti0 = exti0_handler(sender, counter, EXTI);
    exti0.enable();

    loop {
        let value = receiver.recv();
        println!("Handled {:?} interrupts", value);
    }
}
```

## Immediate Priority Ceiling Protocol
threads and interrupt handlers have fixed base priorities, and all priority based
locking primitives have a priority ceiling, which is the maximum priority of any 
thread or an interrupt that can acquire the lock. Acquiring a priority based lock
immediately raises the active priority of the thread or an interrupt to the ceiling
priority of that lock, preventing other threads or interrupts from acquiring the lock.
The priority based locking implements mutual exclusion in the scheduler ready queue.

If the ceiling of the lock is at interrupt priorities, then interrupts are blocked
up to that priority. Higher priority interrupts are allowed to execute, but since
they are above the ceiling, they are never allowed to acquire the lock. The benefit
from this, is that whenever interrupt handler executes, it may access priority lock
protected data; if a thread or lower priority interrupt handler is holding the lock,
the interrupt handler is not allowed to execute in the first place. The downside is
that all locking primitives such as mutexes and condition variables, must have the
ceiling priority specified by the developer.

Violating the priority ceiling protocol by trying to acquire a lock from a thread
or an interrupt handler with higher priority than the lock ceiling priority, will
lead to a runtime error.

## Synchronization primitives
  -  Locks: `InterruptLock`, `PreemptLock` and `CeilingLock`
  - `Mutex`
  - `Condvar`
  - `Channel`
  - others TBD

## Simulator
The `scars-khal-sim` crate implements a pthreads based simulator on host. Interrupt
simulation has not yet been implemented.

## Running tests
### khal-e310x
Configuration in .cargo/config.toml is for QEMU.  

```
$ cargo test --release --package=scars --features="khal-e310x" --target=riscv32imac-unknown-none-elf
```

### khal-sim
```
$ cargo test --release --package=scars --features="khal-sim" --target=x86_64-unknown-linux-gnu
```

### stm32f4
```
$ cargo test --release --package=scars --features=khal-stm32f4 --target=thumbv7em-none-eabihf
```
