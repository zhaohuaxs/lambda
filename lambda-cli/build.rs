//! Keep 7-Zip's registration objects alive in cargo-linked binaries.
//!
//! sevenzip-src ships the engine as a static library plus a response file for
//! Bern's generated app link. A cargo-built binary links the library instead,
//! where `*Register.cpp` members would be dropped as unreferenced; force the
//! whole archive so the CLI finds every codec.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg-bins=/WHOLEARCHIVE:lambda7z");
    }
}
