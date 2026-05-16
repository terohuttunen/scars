use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let memory_bytes: &[u8] = include_bytes!("memory.x");

    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(memory_bytes)
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=default.lds");
    println!("cargo:rerun-if-changed=build.rs");
}
