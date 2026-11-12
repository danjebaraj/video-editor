//! Synthetic projects used by the selftest.

use anyhow::Result;
use cutline_testkit::gen::{self, FrameIdSource};
use cutline_testkit::FrameIdLayout;
use std::path::{Path, PathBuf};

pub const SRC_W: u32 = 1920;
pub const SRC_H: u32 = 1080;

/// Frame-ID layout used by selftest sources (survives a 16:9 → 9:16 cover crop).
pub fn layout() -> FrameIdLayout {
    FrameIdLayout::centered(SRC_W, 200.0, 64.0)
}

/// Sources: a.mp4 (440 Hz), b.mp4 (660 Hz), c.mp4 (880 Hz); 8 s, 30 fps, 1920×1080.
pub fn sources(dir: &Path) -> Result<Vec<(PathBuf, f64)>> {
    let tc = cutline_deps::toolchain()?;
    let mut v = vec![];
    for (name, color, hz) in [("a.mp4", "0x3060a0", 440.0), ("b.mp4", "0xa06030", 660.0), ("c.mp4", "0x30a060", 880.0)]
    {
        let p = dir.join(name);
        if !p.exists() {
            let mut s = FrameIdSource::new(SRC_W, SRC_H, 30, 8.0);
            s.color = color.into();
            s.tone_hz = Some(hz);
            s.layout = layout();
            gen::frameid_video(tc, &p, &s)?;
        }
        v.push((p, hz));
    }
    let logo = dir.join("logo.png");
    if !logo.exists() {
        gen::still(tc, &logo, 256, 256, "0xFFC400")?;
    }
    let m = dir.join("music.wav");
    if !m.exists() {
        gen::noise_bed(tc, &m, 12.0, 0.25)?;
    }
    Ok(v)
}
