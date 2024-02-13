# SCARS
SCARS is a Real-Time Operating System (RTOS) designed for hard real-time applications,
inspired by the Ada Ravenscar profile. It is a traditional style RTOS with fixed
priority preemptive scheduling, not an async executor. **It is still a work-in-progress,
and is not recommended for production systems.** Currently only RISC-V without FPU, and
pthreads based simulator are implemented. ARM and FPU support are planned.

  - It implements the immediate priority ceiling protocol, and provides an unified
    priority model for tasks and interrupts, which allows developers to use the same
    APIs in tasks and interrupts. Mutex protected data can be accessed from interrupt
    handlers.

  - All tasks and interrupt handlers are statically allocated, yet started at runtime,
    allowing tasks and interrupt handlers to capture variables from the environment that
    they are started in. No need for global static variables.

  - All scheduling operations that depend on the number or order of tasks, are
    executed within a preemption lock that allows interrupts, minimizing the
    maximum interrupt latency. Interrupt latency is independent from scheduling
    and number of tasks.

  - Any priority based synchronization primitives that are below a priority ceiling,
    will not prevent higher priority tasks or interrupts from executing.

## Tasks
The entry task is defined with `entry` attribute for a function. There can be only
one entry task. Tasks can never exit.
```rust
#[scars::entry(name = "main", priority = 1, stack_size = 2048)]
fn main() {
    loop {}
}
```
Other tasks are created with the `make_task!` macro, which statically allocates the
task state, including the task stack.
```rust
const TASK_PRIO: TaskPriority = 2;
const TASK_STACK_SIZE: usize = 1024; 
const CHANNEL_CEILING: AnyPriority = Priority::any_task_priority(TASK_PRIO);
const CHANNEL_CAPACITY: usize = 1;

#[scars::entry(name = "main", priority = 1, stack_size = 2048)]
fn main() {
    let task = make_task!("task", TASK_PRIO, TASK_STACK_SIZE);
    let (sender, receiver) = make_channel!(u32, CHANNEL_CAPACITY, CHANNEL_CEILING);

    task.start(move || {
        // Task closure captures `sender`
        let mut counter: u32 = 0;
        loop {
            sender.send(counter);
            counter += 1;
        }
    });

    loop {
        let value = receiver.recv();
        println!("Received {:?}", value);
    }
}
```

### Idle task
The kernel creates an internal idle task to be run when no other task is ready.
The idle task is special in that it does not follow the priority ceiling protocol,
and can run even when higher priority tasks hold locks.

To run custom code in the idle task, the user can define the idle task hook with
the `idle_task_hook` attribute:
```rust
#[scars::idle_task_hook]
fn idle_task_hook() {
    // do something when idle
}
```
The hook-function can use `InterruptLock` and `PreemptLock`; however, due to being exempt
from the priority ceiling protocol, it may not acquire a `CeilingLock`. An attempt to
acquire a `CeilingLock` from the idle task will result in a runtime error.

## Interrupt Handlers
Interrupt handlers are created similarly to tasks with `make_interrupt_handler!` macro.
```rust
use scars::pac;
const INTERRUPT_PRIO: InterruptPriority = 2;
const CHANNEL_CEILING: AnyPriority = Priority::any_interrupt_priority(INTERRUPT_PRIO);
const CHANNEL_CAPACITY: usize = 10;

#[scars::entry(name = "main", priority = 1, stack_size = 2048)]
fn main() {
    let uart_handler = make_interrupt_handler!(pac::Interrupt::UART1, INTERRUPT_PRIO);
    let (sender, receiver) = make_channel!(u32, CHANNEL_CAPACITY, CHANNEL_CEILING);
    let mut counter: u32 = 0;

    uart_handler.attach(move || {
        // Interrupt handler closure captures `sender` and `counter`
        counter += 1;
        // Must use non-blocking `try_send` in interrupt
        let _ = sender.try_send(counter);
    });
    
    uart_handler.enable_interrupt();

    loop {
        let value = receiver.recv();
        println!("Handled {:?} interrupts", value);
    }
}
```

## Immediate Priority Ceiling Protocol
Tasks and interrupt handlers have fixed base priorities, and all priority based
locking primitives have a priority ceiling, which is the maximum priority of any 
task or an interrupt that can acquire the lock. Acquiring a priority based lock
immediately raises the active priority of the task or an interrupt to the ceiling
priority of that lock, preventing other tasks or interrupts from acquiring the lock.
The priority based locking implements mutual exclusion in the scheduler ready queue.

If the ceiling of the lock is at interrupt priorities, then interrupts are blocked
up to that priority. Higher priority interrupts are allowed to execute, but since
they are above the ceiling, they are never allowed to acquire the lock. The benefit
from this, is that whenever interrupt handler executes, it may access priority lock
protected data; if a task or lower priority interrupt handler is holding the lock,
the interrupt handler is not allowed to execute in the first place. The downside is
that all locking primitives such as mutexes and condition variables, must have the
ceiling priority specified by the developer.

Violating the priority ceiling protocol by trying to acquire a lock from a task
or an interrupt handler with higher priority than the lock ceiling priority, will
lead to a runtime error.

## Synchronization primitives
  -  Locks: `InterruptLock`, `PreemptLock` and `CeilingLock`
  - `Mutex`
  - `Condvar`
  - `Channel`
  - others TBD

## Simulator
The `hal-std` crate implements a pthreads based simulator on host. Interrupt
simulation has not yet been implemented.

## Running tests
### hal-e310x
Configuration in .cargo/config.toml is for QEMU.  

```
$ cargo test --release --package=scars --features="hal-e310x" --target=riscv32imac-unknown-none-elf
```

### hal-std
```
$ cargo test --release --package=scars --features="hal-std" --target=x86_64-unknown-linux-gnu
```