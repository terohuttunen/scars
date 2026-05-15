use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());

    // Pick the per-chip memory layout. The on-disk name is `memory-<chip>.x`
    // but cortex-m-rt's `link.x` includes `memory.x`, so we copy the
    // selected one under that fixed name into OUT_DIR.
    let memory_bytes: &[u8] = if cfg!(feature = "stm32f446") {
        include_bytes!("memory-f446.x")
    } else {
        // Default and the existing stm32f429 path.
        include_bytes!("memory-f429.x")
    };

    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(memory_bytes)
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    println!("cargo:rerun-if-changed=memory-f429.x");
    println!("cargo:rerun-if-changed=memory-f446.x");
    println!("cargo:rerun-if-changed=default.lds");
    println!("cargo:rerun-if-changed=build.rs");
}
