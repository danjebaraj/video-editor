//! Embeds the application manifest (long paths, per-monitor DPI v2, common controls 6, Windows
//! 10/11) and the icon into the Windows binaries. Nothing to do on other platforms.

fn main() {
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-changed=resources/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("resources/app.rc", embed_resource::NONE).manifest_required().unwrap();
    }
}
