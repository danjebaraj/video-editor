//! Target presets (§8): geometry, fps, encoder settings, safe zones.

use crate::model::{SafeZone, TargetName};
use crate::time::Fps;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TargetSpec {
    pub name: TargetName,
    pub width: u32,
    pub height: u32,
    pub fps: Fps,
    pub crf: u32,
    pub max_duration: Option<f64>,
    pub safe: SafeZone,
    /// Default caption max chars/line.
    pub max_chars: u32,
    /// Loudness range ceiling (LU).
    pub lra_max: f64,
    /// Expected video bitrate range (kb/s) for the QA info check.
    pub bitrate_kbps: (u32, u32),
    /// `profile: screencast` (set from the project, not the preset).
    pub screencast: bool,
    /// 4:4:4 chroma deliverable (set from the project / export, not the preset).
    pub chroma444: bool,
}

impl TargetSpec {
    pub fn is_vertical(&self) -> bool {
        self.height > self.width
    }
    pub fn aspect(&self) -> f64 {
        self.width as f64 / self.height as f64
    }
    /// Safe rectangle in pixels (x, y, w, h).
    pub fn safe_rect(&self) -> (f64, f64, f64, f64) {
        let (w, h) = (self.width as f64, self.height as f64);
        let x = self.safe.left * w;
        let y = self.safe.top * h;
        (x, y, w - x - self.safe.right * w, h - y - self.safe.bottom * h)
    }
}

/// Resolve a preset. `fps` overrides; `source_fps` is used by long-form targets that follow the
/// source. `custom` uses `w`/`h`.
pub fn resolve(
    name: TargetName,
    fps: Option<Fps>,
    source_fps: Option<Fps>,
    w: Option<u32>,
    h: Option<u32>,
) -> TargetSpec {
    let follow = |default: Fps| {
        fps.or(source_fps.filter(|f| {
            [24.0, 25.0, 30.0, 50.0, 60.0, 23.976, 29.97, 59.94].iter().any(|x| (f.as_f64() - x).abs() < 0.01)
        }))
        .unwrap_or(default)
    };
    let shorts_safe = SafeZone { top: 0.10, bottom: 0.22, left: 0.06, right: 0.16 };
    let wide_safe = SafeZone { top: 0.05, bottom: 0.08, left: 0.05, right: 0.05 };
    match name {
        TargetName::Shorts => TargetSpec {
            name,
            width: 1080,
            height: 1920,
            fps: fps.unwrap_or(Fps::new(30, 1)),
            crf: 18,
            max_duration: Some(180.0),
            safe: shorts_safe,
            max_chars: 22,
            lra_max: 7.0,
            bitrate_kbps: (2_000, 40_000),
            screencast: false,
            chroma444: false,
        },
        TargetName::Long1080p => TargetSpec {
            name,
            width: 1920,
            height: 1080,
            fps: follow(Fps::new(30, 1)),
            crf: 18,
            max_duration: None,
            safe: wide_safe,
            max_chars: 42,
            lra_max: 11.0,
            bitrate_kbps: (2_000, 40_000),
            screencast: false,
            chroma444: false,
        },
        TargetName::Long4k => TargetSpec {
            name,
            width: 3840,
            height: 2160,
            fps: follow(Fps::new(30, 1)),
            crf: 18,
            max_duration: None,
            safe: wide_safe,
            max_chars: 42,
            lra_max: 11.0,
            bitrate_kbps: (8_000, 120_000),
            screencast: false,
            chroma444: false,
        },
        TargetName::Square => TargetSpec {
            name,
            width: 1080,
            height: 1080,
            fps: fps.unwrap_or(Fps::new(30, 1)),
            crf: 18,
            max_duration: None,
            safe: SafeZone { top: 0.06, bottom: 0.10, left: 0.06, right: 0.06 },
            max_chars: 26,
            lra_max: 9.0,
            bitrate_kbps: (1_500, 30_000),
            screencast: false,
            chroma444: false,
        },
        TargetName::Custom => {
            let (w, h) = (w.unwrap_or(1920) & !1, h.unwrap_or(1080) & !1);
            TargetSpec {
                name,
                width: w,
                height: h,
                fps: follow(Fps::new(30, 1)),
                crf: 18,
                max_duration: None,
                safe: if h > w { shorts_safe } else { wide_safe },
                max_chars: if h > w { 22 } else { 42 },
                lra_max: 11.0,
                bitrate_kbps: (500, 120_000),
                screencast: false,
                chroma444: false,
            }
        }
    }
}

pub fn target_str(t: TargetName) -> &'static str {
    match t {
        TargetName::Shorts => "shorts",
        TargetName::Long1080p => "long_1080p",
        TargetName::Long4k => "long_4k",
        TargetName::Square => "square",
        TargetName::Custom => "custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets() {
        let s = resolve(TargetName::Shorts, None, None, None, None);
        assert_eq!((s.width, s.height, s.fps), (1080, 1920, Fps::new(30, 1)));
        let (x, y, w, h) = s.safe_rect();
        assert!((y - 192.0).abs() < 1e-9 && (h - (1920.0 - 192.0 - 422.4)).abs() < 1e-6 && x > 0.0 && w < 1080.0);
        let l = resolve(TargetName::Long1080p, None, Some(Fps::new(25, 1)), None, None);
        assert_eq!(l.fps, Fps::new(25, 1));
        let l = resolve(TargetName::Long1080p, None, Some(Fps::new(17, 1)), None, None);
        assert_eq!(l.fps, Fps::new(30, 1));
    }
}
