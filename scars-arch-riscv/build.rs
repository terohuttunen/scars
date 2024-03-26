fn main() {
    println!("cargo:rerun-if-changed=trap.S");
    println!("cargo:rerun-if-changed=build.rs");
}
