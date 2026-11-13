//! Voice processing chain presets (ffmpeg filter chains, comma-joined).

/// Filter chain for a preset name. `None` for unknown names; `Some("")` for raw.
pub fn voice_chain(name: &str, arnndn_model: Option<&str>) -> Option<String> {
    let s = match name {
        // highpass 80 → moderate denoise → de-ess → presence EQ (+2 dB @ 3–5 kHz, −2 dB @ 250–400 Hz)
        // → ~3:1 compressor → limiter
        "clean_voice" => [
            "highpass=f=80:poles=2",
            "afftdn=nr=10:nf=-50:tn=1",
            "deesser=i=0.4:m=0.5:f=0.5:s=o",
            "equalizer=f=4000:t=q:w=1.0:g=2",
            "equalizer=f=320:t=q:w=1.2:g=-2",
            "acompressor=threshold=-20dB:ratio=3:attack=10:release=150:makeup=2",
            "alimiter=limit=0.89:attack=5:release=50:level=disabled",
        ]
        .join(","),
        // warmer: gentle low shelf, softer top, slower compression
        "podcast_warm" => [
            "highpass=f=70:poles=2",
            "afftdn=nr=8:nf=-50:tn=1",
            "lowshelf=f=180:g=2",
            "equalizer=f=3000:t=q:w=1.0:g=1",
            "highshelf=f=9000:g=-1.5",
            "deesser=i=0.3:m=0.5:f=0.5:s=o",
            "acompressor=threshold=-22dB:ratio=2.5:attack=20:release=250:makeup=2",
            "alimiter=limit=0.89:attack=5:release=60:level=disabled",
        ]
        .join(","),
        "raw" => String::new(),
        "arnndn" => {
            let m = arnndn_model?;
            format!("highpass=f=80,arnndn=m={m},acompressor=threshold=-20dB:ratio=3:attack=10:release=150:makeup=2,alimiter=limit=0.89:level=disabled")
        }
        _ => return None,
    };
    Some(s)
}

/// Music bed default: gentle highpass to clean rumble; nothing else.
pub fn music_chain() -> String {
    "highpass=f=30".into()
}

/// Atempo chain for arbitrary speeds (each stage within 0.5–2).
pub fn atempo(speed: f64) -> Vec<String> {
    let mut v = vec![];
    let mut s = speed;
    while s >= 2.0 - 1e-9 && (s - 1.0).abs() > 1e-9 {
        v.push("atempo=2".to_string());
        s /= 2.0;
    }
    while s <= 0.5 + 1e-9 {
        v.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    if (s - 1.0).abs() > 1e-9 {
        v.push(format!("atempo={s:.6}"));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets() {
        assert!(voice_chain("clean_voice", None).unwrap().contains("deesser"));
        assert_eq!(voice_chain("raw", None).unwrap(), "");
        assert!(voice_chain("arnndn", None).is_none());
        assert!(voice_chain("nope", None).is_none());
        assert_eq!(atempo(4.0), vec!["atempo=2", "atempo=2"]);
        assert_eq!(atempo(0.25), vec!["atempo=0.5", "atempo=0.5"]);
    }
}
