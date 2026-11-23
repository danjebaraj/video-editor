// Builds in the fingerprint of the cutline source next to this crate: the cutline this Video
// Studio expects. A cutline that reports another fingerprint is out of step with it.
include!("../cutline-cli/source_fingerprint.rs");

fn main() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace = manifest.parent().and_then(|p| p.parent()).unwrap().to_path_buf();
    let (fingerprint, watch) = cutline_source_fingerprint(&workspace);
    for p in watch {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    println!("cargo:rustc-env=VIDEO_STUDIO_CUTLINE_SOURCE={fingerprint}");
}
