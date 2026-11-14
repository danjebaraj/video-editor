//! Stable identifiers: `prj_…`, `run_…`, `out_…`, and human-readable media IDs.

use std::collections::HashSet;

const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// A sortable random ID: 10 characters of time (ms) and 10 random, Crockford base 32.
pub fn new_id(prefix: &str) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut rnd = [0u8; 8];
    if getrandom::fill(&mut rnd).is_err() {
        rnd = (ms ^ std::process::id() as u64).to_le_bytes();
    }
    let r = u64::from_le_bytes(rnd);
    let mut s = String::with_capacity(prefix.len() + 21);
    s.push_str(prefix);
    s.push('_');
    for i in (0..10).rev() {
        s.push(ALPHABET[((ms >> (i * 5)) & 31) as usize] as char);
    }
    for i in (0..10).rev() {
        s.push(ALPHABET[((r >> (i * 5)) & 31) as usize] as char);
    }
    s
}

/// Lower-case ASCII slug: letters, digits and single dashes.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// `prefix-slug`, shortened and made unique against `taken` with `-2`, `-3`, …
pub fn unique_slug_id(prefix: &str, name: &str, taken: &HashSet<String>) -> String {
    let mut base = slug(name);
    if base.len() > 32 {
        base.truncate(32);
        while base.ends_with('-') {
            base.pop();
        }
    }
    if base.is_empty() {
        base = "item".into();
    }
    let first = format!("{prefix}-{base}");
    if !taken.contains(&first) {
        return first;
    }
    let mut n = 2;
    loop {
        let c = format!("{prefix}-{base}-{n}");
        if !taken.contains(&c) {
            return c;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_prefixed() {
        let a = new_id("prj");
        let b = new_id("prj");
        assert!(a.starts_with("prj_") && a.len() == 24);
        assert_ne!(a, b);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Intro Take 2.mov"), "intro-take-2-mov");
        let mut taken = HashSet::new();
        taken.insert("m-hero".to_string());
        assert_eq!(unique_slug_id("m", "Hero", &taken), "m-hero-2");
        assert_eq!(unique_slug_id("m", "???", &taken), "m-item");
    }
}
