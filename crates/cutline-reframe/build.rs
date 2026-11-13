//! macOS + `onnx`: link clang's compiler-rt builtins.
//!
//! The prebuilt ONNX Runtime (CoreML provider) calls `__isPlatformVersionAtLeast` (emitted for
//! Objective-C `@available`). Rust's std defines that symbol, but under cross-crate LTO
//! (`lto = "thin"` in the release profile) it is internalized, so release links failed with
//! "Undefined symbols: ___isPlatformVersionAtLeast". Linking `libclang_rt.osx.a` (an archive,
//! so its member is only pulled in when the symbol is still undefined) resolves it in every
//! profile.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let onnx = std::env::var_os("CARGO_FEATURE_ONNX").is_some();
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if !(onnx && macos) {
        return;
    }
    let cc = std::env::var("CC").unwrap_or_else(|_| "clang".into());
    let Some(dir) = Command::new(&cc)
        .arg("--print-resource-dir")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()).join("lib/darwin"))
        .filter(|d| d.join("libclang_rt.osx.a").is_file())
    else {
        println!("cargo:warning=libclang_rt.osx.a not found via `{cc} --print-resource-dir`; release (LTO) links of the onnx feature may fail");
        return;
    };
    println!("cargo:rustc-link-search=native={}", dir.display());
    // `-bundle`: pass the archive to the final link instead of copying it into the rlib.
    println!("cargo:rustc-link-lib=static:-bundle=clang_rt.osx");
}
