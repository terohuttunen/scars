use core::sync::atomic::{AtomicU8, Ordering};

const UNSTARTED: u8 = 0;
const STARTED: u8 = 1;
const COMPLETED: u8 = 2;

#[derive(Debug)]
pub struct Once {
    state: AtomicU8,
}

impl Once {
    pub const fn new() -> Once {
        Once {
            state: AtomicU8::new(UNSTARTED),
        }
    }

    pub fn call_once<F: FnOnce()>(&self, f: F) {
        if let Ok(_) =
            self.state
                .compare_exchange(UNSTARTED, STARTED, Ordering::AcqRel, Ordering::Acquire)
        {
            f();
            self.state.store(COMPLETED, Ordering::Release);
        }
    }

    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == COMPLETED
    }
}
