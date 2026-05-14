//! defmt global logger for the simulator.
//!
//! Each `defmt::{println, info, ...}` call enters this logger as an
//! `acquire` -> one-or-more `write` -> `release` sequence. The frame is
//! encoded by `::defmt::Encoder`, the encoded bytes are fed into a
//! `defmt_decoder::StreamDecoder` parsed from the running process's own ELF,
//! and the decoded text is written to stdout on `release`.
//!
//! For this to work, the sim binary is linked as **non-PIE**
//! (`boards/sim.toml` sets `-Crelocation-model=static` and `-no-pie`). defmt
//! emits the format-string tag as `&SYMBOL as u16` — only the low 16 bits of
//! the static's address. With ASLR/PIE active those low bits include random
//! page-offset nibbles, so the runtime tag doesn't match the link-time
//! symbol value `defmt-decoder` reads from the ELF. Non-PIE keeps the runtime
//! address equal to the link-time address.

use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use defmt_decoder::{DecodeError, StreamDecoder, Table};
use std::fs::File;
use std::io::Write;
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::sync::{Mutex, MutexGuard, OnceLock};

struct Inner {
    encoder: ::defmt::Encoder,
    decoder: Box<dyn StreamDecoder + Send + Sync + 'static>,
}

fn state() -> &'static Mutex<Inner> {
    static TABLE: OnceLock<Table> = OnceLock::new();
    static STATE: OnceLock<Mutex<Inner>> = OnceLock::new();
    STATE.get_or_init(|| {
        let table: &'static Table = TABLE.get_or_init(|| {
            let exe = std::env::current_exe().expect("defmt: cannot read current_exe");
            let elf = std::fs::read(&exe).expect("defmt: cannot read sim ELF");
            Table::parse(&elf).ok().flatten().expect(
                "defmt: no .defmt section in sim ELF — \
                 check that sim-defmt.lds is linked",
            )
        });
        Mutex::new(Inner {
            encoder: ::defmt::Encoder::new(),
            decoder: table.new_stream_decoder(),
        })
    })
}

// Slot holding the locked MutexGuard between `acquire` and `release`.
// Serialisation is enforced by the Mutex itself — only the thread that
// obtained the guard ever touches this slot before the guard is dropped.
static GUARD_SLOT: SyncUnsafeCell<MaybeUninit<MutexGuard<'static, Inner>>> =
    SyncUnsafeCell::new(MaybeUninit::uninit());

unsafe fn guard_mut() -> &'static mut Inner {
    unsafe { (*GUARD_SLOT.get()).assume_init_mut() }
}

#[::defmt::global_logger]
struct Logger;

unsafe impl ::defmt::Logger for Logger {
    fn acquire() {
        let guard = state().lock().expect("defmt logger mutex poisoned");
        // Relabel the guard's borrow to `'static`; the underlying Mutex
        // lives in a `'static` OnceLock, so this is sound.
        let guard: MutexGuard<'static, Inner> = unsafe { core::mem::transmute(guard) };
        unsafe { (*GUARD_SLOT.get()).write(guard) };
        let Inner { encoder, decoder } = unsafe { guard_mut() };
        encoder.start_frame(|b| decoder.received(b));
    }

    unsafe fn write(bytes: &[u8]) {
        let Inner { encoder, decoder } = unsafe { guard_mut() };
        encoder.write(bytes, |b| decoder.received(b));
    }

    unsafe fn flush() {
        // Output goes straight to fd 1 below — nothing to flush here.
    }

    unsafe fn release() {
        let inner = unsafe { guard_mut() };
        {
            let Inner { encoder, decoder } = &mut *inner;
            encoder.end_frame(|b| decoder.received(b));
        }
        // Write directly to fd 1; `std::io::stdout()` adds buffering that
        // doesn't reliably flush before SIGTERM for short-lived sim runs.
        // The rzcobs leading sentinel zero shows up as `Malformed` (empty
        // frame) — treat it as a resync marker, not an abort.
        let mut out = ManuallyDrop::new(unsafe { File::from_raw_fd(1) });
        loop {
            match inner.decoder.decode() {
                Ok(frame) => {
                    let _ = writeln!(out, "{}", frame.display(false));
                }
                Err(DecodeError::UnexpectedEof) => break,
                Err(DecodeError::Malformed) => continue,
            }
        }
        let guard = unsafe { (*GUARD_SLOT.get()).assume_init_read() };
        drop(guard);
    }
}

#[::defmt::panic_handler]
fn defmt_panic() -> ! {
    panic!("defmt panic");
}
