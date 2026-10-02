fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Cargo target cfg expressions cannot select flags by debug_assertions.
    // Use the target profile here; build scripts themselves run on the host.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("emscripten")
        && std::env::var("PROFILE").as_deref() == Ok("debug")
    {
        println!("cargo:rustc-link-arg-bin=snow=-sASSERTIONS=2");
        println!("cargo:rustc-link-arg-bin=snow=-sSTACK_OVERFLOW_CHECK=2");
    }
}
