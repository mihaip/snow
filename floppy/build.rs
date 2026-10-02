fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("emscripten") {
        println!("cargo:rerun-if-changed=build.rs");
        return;
    }
    println!("cargo:rerun-if-changed=../.git/HEAD");

    built::write_built_file().expect("Failed to acquire build-time information");
}
