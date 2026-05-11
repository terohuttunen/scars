use scars_fault::*;

#[repr(u8)]
#[derive(PartialEq, Eq, Copy, Clone, Fault)]
pub enum SimulatorErrorKind {
    #[fault("Mutex lock failed for mutex at {mutex_ptr:?}")]
    MutexLockFailed {
        mutex_ptr: *const libc::pthread_mutex_t,
    } = 1,
    #[fault("Condition wait failed for condition at {cond_ptr:?}")]
    CondWaitFailed {
        cond_ptr: *const libc::pthread_cond_t,
    } = 2,
    #[fault("Mutex unlock failed for mutex at {mutex_ptr:?}")]
    MutexUnlockFailed {
        mutex_ptr: *const libc::pthread_mutex_t,
    } = 3,
    #[fault("Thread stack initialization failed for thread '{name}' with stack size {stack_size}")]
    ThreadStackInitFailed {
        name: &'static str,
        stack_size: usize,
    } = 4,
    #[fault("Failed to read clock {clock_id}")]
    ClockReadFailed { clock_id: libc::clockid_t } = 5,
    #[fault("Failed to set signal mask for signal {signal}")]
    SignalMaskFailed { signal: libc::c_int } = 6,
    #[fault("Failed to set signal handler for signal {signal}")]
    SignalHandlerFailed { signal: libc::c_int } = 7,
    #[fault("Unhandled exception: {exception_type}")]
    UnhandledException { exception_type: &'static str } = 8,
    #[fault("Unknown error")]
    Unknown = 255,
}

#[derive(Fault)]
#[fault("Simulator error: {kind:?}")]
pub struct SimulatorError {
    kind: SimulatorErrorKind,
}

impl SimulatorError {
    pub fn new(kind: SimulatorErrorKind) -> SimulatorError {
        SimulatorError { kind }
    }
}

#[derive(FaultContext)]
#[fault("simulator pid {pid}")]
pub struct SimContext {
    pub pid: libc::pid_t,
}
