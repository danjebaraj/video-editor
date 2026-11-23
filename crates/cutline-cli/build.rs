// Builds the fingerprint of cutline's source into the binary (`cutline build-info`), so Video
// Studio can tell whether this cutline was built from the sources it expects.
include!("source_fingerprint.rs");

fn main() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace = manifest.parent().and_then(|p| p.parent()).unwrap().to_path_buf();
    let (fingerprint, watch) = cutline_source_fingerprint(&workspace);
    for p in watch {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    println!("cargo:rustc-env=CUTLINE_SOURCE={fingerprint}");
}
