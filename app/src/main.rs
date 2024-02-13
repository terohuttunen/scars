#![no_std]
#![no_main]
#![feature(panic_info_message)]
#![feature(sync_unsafe_cell)]
#![feature(type_alias_impl_trait)]
use scars::pac;
use scars::sync::Shared;
use scars::time::{Duration, Instant};
use scars::{kernel::print_tasks, make_channel, make_interrupt_handler, make_shared, make_task};
use scars::{AnyPriority, Priority, Task, TaskRef};

// Simulator needs bigger stacks
#[cfg(feature = "hal-std")]
const APP_TASK_STACK_SIZE: usize = 2048 * 8;
#[cfg(feature = "hal-std")]
const IO_TASK_STACK_SIZE: usize = 2048 * 8;

#[cfg(not(feature = "hal-std"))]
const APP_TASK_STACK_SIZE: usize = 2048;
#[cfg(not(feature = "hal-std"))]
const IO_TASK_STACK_SIZE: usize = 2048;

const UART1_INTERRUPT_PRIO: u8 = 1;

const APP_TASK_PRIO: u8 = 3;
const IO_TASK_PRIO: u8 = 2;

const APP_IO_CEILING_PRIO: AnyPriority = 3;
const APP_IO_UART_CEILING_PRIO: AnyPriority = Priority::interrupt_priority(1).into_any();

const CHANNEL_PRIO: AnyPriority = APP_IO_CEILING_PRIO;
const CHANNEL_CAPACITY: usize = 10;

/*
#[scars::trace_task_new]
fn trace_task_new(task: TaskRef) {
    scars::printkln!("Task new {:?}", task.name());
}

#[scars::trace_task_ready_begin]
fn trace_task_ready_begin(task: TaskRef) {
    scars::printkln!("Task new {:?} is ready", task.name());
}

#[scars::trace_system_idle]
fn trace_system_idle() {
    scars::printkln!("System Idle");
}
*/

#[cfg_attr(
    feature = "hal-std",
    scars::entry(name = "main", priority = 1, stack_size = 16384)
)]
#[cfg_attr(
    not(feature = "hal-std"),
    scars::entry(name = "main", priority = 1, stack_size = 1024)
)]
pub fn main() {
    scars::printkln!("In main, starting tasks...");
    //let pac::Peripherals { UART1, .. } = unsafe { pac::Peripherals::steal() };
    let shared = make_shared!(APP_IO_UART_CEILING_PRIO, 10u32);

    let producer_task = make_task!("producer", APP_TASK_PRIO, APP_TASK_STACK_SIZE);
    let consumer_task = make_task!("consumer", IO_TASK_PRIO, IO_TASK_STACK_SIZE);
    let uart1_handler = make_interrupt_handler!(pac::Interrupt::UART1, UART1_INTERRUPT_PRIO);
    let (sender, receiver) = make_channel!(u64, CHANNEL_CAPACITY, CHANNEL_PRIO);

    let sender2 = sender.clone();

    let mut count = 0;

    producer_task.start(move || {
        let mut time: Instant = Instant::now();
        loop {
            scars::printkln!("[producer]: sending time {}", count);
            let _ = sender.send(count);
            time = time + Duration::from_secs(1);
            scars::printkln!("[producer]: going to sleep until {:?}", time);
            scars::delay_until(time);
            count += 1;
        }
    });

    consumer_task.start(move || loop {
        scars::printkln!("[consumer]: queueing for more data");
        let count = receiver.recv();
        scars::printkln!("[consumer]: received count {}", count);
        print_tasks();
    });

    uart1_handler.attach(move || {
        count += 1;
        let _ = sender2.try_send(count);
    });
    uart1_handler.enable_interrupt();

    scars::printkln!("Tasks started");
    loop {
        scars::idle();
    }
}
