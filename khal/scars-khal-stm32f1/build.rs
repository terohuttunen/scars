use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let memory_bytes: &[u8] = if cfg!(feature = "stm32f103") {
        include_bytes!("memory-f103rb.x")
    } else {
        // Fall back to the F103RB layout if no chip feature is selected;
        // the lib.rs cfg-gate will then produce a clearer error.
        include_bytes!("memory-f103rb.x")
    };

    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(memory_bytes)
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    println!("cargo:rerun-if-changed=memory-f103rb.x");
    println!("cargo:rerun-if-changed=default.lds");
    println!("cargo:rerun-if-changed=build.rs");
}
