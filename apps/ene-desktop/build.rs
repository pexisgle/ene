//! Build script for `ene-desktop`.
//!
//! On Linux the `voice` feature pulls in both `whisper-rs-sys` and
//! `llama-cpp-sys-2`, each of which vendors its own copy of the `ggml`
//! C library. When the final binary links both, the duplicate `ggml_*`
//! symbols collide. Passing `--allow-multiple-definition` to the linker
//! lets the first definition win instead of failing the link. This is
//! scoped to `ene-desktop` (the only binary that links both crates) and
//! to Linux (the only platform where the GNU linker is used); the
//! global `.cargo/config.toml` flag was removed so other crates are
//! unaffected.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Build scripts are compiled for the host, so `#[cfg(target_os)]`
    // would test the host OS. Cargo exposes the *target* OS through
    // `CARGO_CFG_TARGET_OS`, which is what we want here.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
    }
}
