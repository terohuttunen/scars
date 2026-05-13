use crate::Simulator;
use crate::context::CURRENT_THREAD_CONTEXT;
use crate::error::SimulatorErrorKind;
use crate::signal::{ALARM_SIGNAL, SIMULATOR_CLOCK, SYSCALL_SIGNAL, install_handler};
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;
use scars_fault::*;
use scars_khal::*;

pub const TIMER_FREQ_HZ: u64 = 10_000_000;

pub struct VirtualTimer {
    pub(crate) wait: UnsafeCell<libc::pthread_cond_t>,
    pub(crate) wait_lock: UnsafeCell<libc::pthread_mutex_t>,
    pub(crate) wait_until: UnsafeCell<Option<libc::timespec>>,
    thread_id: libc::pthread_t,
}

impl VirtualTimer {
    pub fn init(timer_ptr: *mut Self) {
        unsafe {
            // ALARM_SIGNAL handler additionally masks SYSCALL_SIGNAL while running.
            install_handler(ALARM_SIGNAL, &[SYSCALL_SIGNAL], 0);

            let mut cond_attr = MaybeUninit::uninit();
            libc::pthread_condattr_init(cond_attr.as_mut_ptr());
            libc::pthread_condattr_setclock(cond_attr.as_mut_ptr(), SIMULATOR_CLOCK);
            libc::pthread_cond_init((*timer_ptr).wait.get(), cond_attr.as_ptr());
            (*timer_ptr).wait_lock = UnsafeCell::new(libc::PTHREAD_MUTEX_INITIALIZER);
            (*timer_ptr).wait_until = UnsafeCell::new(None);

            let mut attr = MaybeUninit::uninit();
            libc::pthread_attr_init(attr.as_mut_ptr());
            libc::pthread_create(
                &raw mut (*timer_ptr).thread_id,
                attr.as_ptr(),
                timer_thread,
                timer_ptr as *mut libc::c_void,
            );
        }
    }

    pub(crate) fn timespec_to_ticks(time: libc::timespec) -> u64 {
        (time.tv_sec as u64) * TIMER_FREQ_HZ + (time.tv_nsec as u64) * TIMER_FREQ_HZ / 1_000_000_000
    }

    pub(crate) fn ticks_to_timespec(ticks: u64) -> libc::timespec {
        libc::timespec {
            tv_sec: (ticks / TIMER_FREQ_HZ) as i64,
            tv_nsec: ((ticks % TIMER_FREQ_HZ) * (1_000_000_000 / TIMER_FREQ_HZ)) as i64,
        }
    }

    pub(crate) fn handle_alarm() {
        unsafe { Simulator::kernel_wakeup_handler() };
    }
}

extern "C" fn timer_thread(arg: *mut libc::c_void) -> *mut libc::c_void {
    let timer = unsafe { &*(arg as *const VirtualTimer) };

    unsafe {
        libc::pthread_mutex_lock(timer.wait_lock.get());

        loop {
            if let Some(time) = *timer.wait_until.get() {
                let mut now = core::mem::MaybeUninit::uninit();
                if libc::clock_gettime(SIMULATOR_CLOCK, now.as_mut_ptr()) != 0 {
                    fault!(SimulatorErrorKind::ClockReadFailed {
                        clock_id: SIMULATOR_CLOCK
                    });
                }

                let now_ticks = VirtualTimer::timespec_to_ticks(now.assume_init());
                let time_ticks = VirtualTimer::timespec_to_ticks(time);
                if now_ticks >= time_ticks {
                    // Timeout is in the past
                    (*timer.wait_until.get()) = None;
                    let context = CURRENT_THREAD_CONTEXT.load(Ordering::SeqCst);
                    libc::pthread_sigqueue(
                        (*context).thread_id,
                        ALARM_SIGNAL,
                        libc::sigval {
                            sival_ptr: core::ptr::null_mut(),
                        },
                    );
                    continue;
                }

                if libc::pthread_cond_timedwait(
                    timer.wait.get(),
                    timer.wait_lock.get(),
                    &time as *const _,
                ) == libc::ETIMEDOUT
                {
                    // Timer expired
                    (*timer.wait_until.get()) = None;
                    let context = CURRENT_THREAD_CONTEXT.load(Ordering::SeqCst);
                    libc::pthread_sigqueue(
                        (*context).thread_id,
                        ALARM_SIGNAL,
                        libc::sigval {
                            sival_ptr: core::ptr::null_mut(),
                        },
                    );
                }
            } else {
                // Wait until a new timeout is set
                libc::pthread_cond_wait(timer.wait.get(), timer.wait_lock.get());
            }
        }
    }
}

impl AlarmClockController for Simulator {
    const TICK_FREQ_HZ: u64 = TIMER_FREQ_HZ;

    fn clock_ticks() -> u64 {
        let mut time = core::mem::MaybeUninit::uninit();
        unsafe {
            if libc::clock_gettime(SIMULATOR_CLOCK, time.as_mut_ptr()) != 0 {
                fault!(SimulatorErrorKind::ClockReadFailed {
                    clock_id: SIMULATOR_CLOCK
                });
            }
        }
        let time = unsafe { time.assume_init() };
        VirtualTimer::timespec_to_ticks(time)
    }

    fn set_wakeup(at: Option<u64>) {
        unsafe {
            libc::pthread_mutex_lock(Self::instance().timer.wait_lock.get());

            (*Self::instance().timer.wait_until.get()) =
                at.map(|ticks| VirtualTimer::ticks_to_timespec(ticks));

            libc::pthread_cond_signal(Self::instance().timer.wait.get());
            libc::pthread_mutex_unlock(Self::instance().timer.wait_lock.get());
        }
    }
}
