fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Building the command line from the clap derive takes close to 1 MiB of
    // stack in a debug build, which is all a Windows main thread has, so the
    // binary asks for the 8 MiB Linux and macOS give.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let arg = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => "/STACK:8388608",
            _ => "-Wl,--stack,8388608",
        };
        println!("cargo:rustc-link-arg-bins={arg}");
    }
}
