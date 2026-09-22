fn main() {
    println!("cargo:rerun-if-changed=crates/wamr-sys");
}
