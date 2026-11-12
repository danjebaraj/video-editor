//! ffmpeg filtergraph escaping.
//!
//! A value inside a filter option goes through two unescaping levels: the filtergraph parser
//! (`\ ' [ ] , ;`) and the option parser (`\ ' :`). Windows paths are converted to forward
//! slashes so `C:\x\y.ass` becomes `C\\:/x/y.ass` in the script.

/// Escape a string used as a filter option value (level 1 + level 2).
pub fn filter_value(s: &str) -> String {
    let l1 = s.replace('\\', "\\\\").replace('\'', "\\'").replace(':', "\\:");
    l1.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

/// Escape a filesystem path for use as a filter option value.
pub fn filter_path(p: &std::path::Path) -> String {
    filter_value(&path_fwd(p))
}

/// Path with forward slashes (Windows-safe inside filter strings).
pub fn path_fwd(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Escape text for `drawtext` text= (rarely used; captions go through ASS).
pub fn drawtext_text(s: &str) -> String {
    filter_value(&s.replace('%', "\\%"))
}

/// Format a float compactly with enough precision for frame-accurate timing.
pub fn f(v: f64) -> String {
    let s = format!("{:.6}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn windows_drive_letter() {
        assert_eq!(filter_value("C:/Users/me/sub.ass"), "C\\\\:/Users/me/sub.ass");
        // backslash paths become forward slashes first
        assert_eq!(filter_value(&"C:\\Users\\me\\sub.ass".replace('\\', "/")), "C\\\\:/Users/me/sub.ass");
    }

    #[test]
    fn specials() {
        assert_eq!(filter_value("a,b"), "a\\,b");
        assert_eq!(filter_value("x[1];y"), "x\\[1\\]\\;y");
        assert_eq!(filter_value("it's"), "it\\\\\\'s");
        assert_eq!(filter_value("ü space"), "ü space");
        assert_eq!(path_fwd(Path::new("a\\b\\c")), "a/b/c");
    }

    #[test]
    fn floats() {
        assert_eq!(f(1.0), "1");
        assert_eq!(f(1.25), "1.25");
        assert_eq!(f(1.0 / 3.0), "0.333333");
        assert_eq!(f(-0.0000001), "0");
    }
}
