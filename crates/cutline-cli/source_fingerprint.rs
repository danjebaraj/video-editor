// The fingerprint of cutline's source, shared by two build scripts with `include!`:
// `cutline-cli/build.rs` builds it into cutline (`cutline build-info`), and
// `video-studio-core/build.rs` builds it into Video Studio as the cutline it expects. A cutline
// whose fingerprint differs was built from other sources: older or newer, it isn't the one this
// Video Studio was made with. cutline's version number can't tell them apart, since it rarely
// changes (BUG-deft-lily-344X).
//
// It covers every `crates/cutline-*` crate (Cargo.toml, build.rs, src/) and the shared `assets/`.
// Line endings are normalized, so a Windows checkout gives the same value.

/// Returns the fingerprint (16 hex digits) and the paths cargo should watch.
#[allow(dead_code)]
fn cutline_source_fingerprint(workspace: &std::path::Path) -> (String, Vec<std::path::PathBuf>) {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut watch = Vec::new();
    let mut files = Vec::new();
    let crates = workspace.join("crates");
    let mut names: Vec<String> = std::fs::read_dir(&crates)
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.retain(|n| n.starts_with("cutline-"));
    names.sort();
    for n in &names {
        let c = crates.join(n);
        for f in ["Cargo.toml", "build.rs"] {
            let p = c.join(f);
            watch.push(p.clone());
            if p.is_file() {
                files.push(p);
            }
        }
        let src = c.join("src");
        watch.push(src.clone());
        walk(&src, &mut files);
    }
    let assets = workspace.join("assets");
    watch.push(assets.clone());
    walk(&assets, &mut files);
    watch.push(crates.join("cutline-cli/source_fingerprint.rs"));

    let rel = |p: &std::path::Path| p.strip_prefix(workspace).unwrap_or(p).to_string_lossy().replace('\\', "/");
    let mut keyed: Vec<(String, std::path::PathBuf)> = files.into_iter().map(|p| (rel(&p), p)).collect();
    keyed.sort();
    // FNV-1a, 64 bits: the same on every platform and toolchain
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (key, path) in keyed {
        let mut body = std::fs::read(&path).unwrap_or_default();
        body.retain(|b| *b != b'\r');
        eat(key.as_bytes());
        eat(&[0]);
        eat(&body);
        eat(&[0]);
    }
    (format!("{h:016x}"), watch)
}
