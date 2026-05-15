use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());

    // STM32F051R8 is the only F0 variant the F0Discovery board carries;
    // additional memory maps go behind chip features the same way
    // memory-f446.x sits next to memory-f429.x in the F4 KHAL.
    let memory_bytes: &[u8] = include_bytes!("memory-f051r8.x");

    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(memory_bytes)
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    println!("cargo:rerun-if-changed=memory-f051r8.x");
    println!("cargo:rerun-if-changed=default.lds");
    println!("cargo:rerun-if-changed=build.rs");
}
