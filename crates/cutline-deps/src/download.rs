//! HTTP downloads with sha256 verification and archive extraction.
//! These are the ONLY network calls cutline makes (`cutline deps ...`).

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub fn client() -> Result<reqwest::blocking::Client> {
    Ok(client_builder()?.build()?)
}

fn client_builder() -> Result<reqwest::blocking::ClientBuilder> {
    let mut b = reqwest::blocking::Client::builder()
        .user_agent(concat!("cutline/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(600));
    // Honour an explicit CA bundle (corporate / sandbox proxies).
    for var in ["CUTLINE_CA_BUNDLE", "SSL_CERT_FILE"] {
        if let Some(p) = std::env::var_os(var) {
            if let Ok(pem) = std::fs::read(&p) {
                if let Ok(certs) = reqwest::Certificate::from_pem_bundle(&pem) {
                    for c in certs {
                        b = b.add_root_certificate(c);
                    }
                }
            }
            break;
        }
    }
    Ok(b)
}

/// GET a URL as text.
pub fn get_text(url: &str) -> Result<String> {
    retry(|| get_text_once(url))
}

fn get_text_once(url: &str) -> Result<String> {
    let r = client()?
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;
    if !r.status().is_success() {
        bail!("GET {url}: HTTP {}", r.status());
    }
    Ok(r.text()?)
}

/// Resolve redirects and return the final URL.
pub fn resolve_url(url: &str) -> Result<String> {
    retry(|| resolve_url_once(url))
}

/// Retry transient failures up to 4 times with exponential backoff.
pub fn retry<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let mut delay = std::time::Duration::from_millis(500);
    let mut last = None;
    for attempt in 0..4 {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if format!("{e:#}").contains("sha256 mismatch") => return Err(e),
            Err(e) => {
                tracing::warn!(attempt, "request failed: {e:#}");
                last = Some(e);
                std::thread::sleep(delay);
                delay *= 2;
            }
        }
    }
    Err(last.unwrap())
}

fn resolve_url_once(url: &str) -> Result<String> {
    let mut cur = reqwest::Url::parse(url)?;
    for _ in 0..10 {
        let r = client_builder()?
            .redirect(reqwest::redirect::Policy::none())
            .build()?
            .head(cur.clone())
            .send()
            .with_context(|| format!("HEAD {cur}"))?;
        if r.status().is_redirection() {
            let loc = r
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow::anyhow!("redirect without location"))?;
            cur = cur.join(loc)?;
            continue;
        }
        if !r.status().is_success() {
            bail!("HEAD {cur}: HTTP {}", r.status());
        }
        return Ok(cur.to_string());
    }
    bail!("too many redirects for {url}")
}

/// Download `url` to `dest`, returning the lowercase hex sha256. Verifies against `expected`
/// when given (and deletes the file on mismatch).
pub fn download(url: &str, dest: &Path, expected: Option<&str>) -> Result<String> {
    retry(|| download_once(url, dest, expected))
}

fn download_once(url: &str, dest: &Path, expected: Option<&str>) -> Result<String> {
    tracing::info!(url, dest = %dest.display(), "downloading");
    let mut r = client()?
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;
    if !r.status().is_success() {
        bail!("GET {url}: HTTP {}", r.status());
    }
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut f = std::fs::File::create(dest)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        f.write_all(&buf[..n])?;
    }
    f.flush()?;
    let got = hex::encode(h.finalize());
    if let Some(e) = expected {
        if !e.eq_ignore_ascii_case(&got) {
            let _ = std::fs::remove_file(dest);
            bail!("sha256 mismatch for {url}: expected {e}, got {got}");
        }
    }
    Ok(got)
}

pub fn sha256_file(p: &Path) -> Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

/// Parse a `.sha256` sidecar (`<hex>  <name>` or just `<hex>`).
pub fn parse_sha256_sidecar(s: &str) -> Option<String> {
    let tok = s.split_whitespace().next()?;
    (tok.len() == 64 && tok.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| tok.to_ascii_lowercase())
}

/// Extract an archive (zip, tar.gz/tgz, tar) into `dest`. Returns extracted file paths.
pub fn extract(archive: &Path, dest: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dest)?;
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut out = vec![];
    if name.ends_with(".zip") {
        let f = std::fs::File::open(archive)?;
        let mut z = zip::ZipArchive::new(f)?;
        for i in 0..z.len() {
            let mut e = z.by_index(i)?;
            let Some(rel) = e.enclosed_name() else {
                continue;
            };
            let p = dest.join(rel);
            if e.is_dir() {
                std::fs::create_dir_all(&p)?;
                continue;
            }
            if let Some(par) = p.parent() {
                std::fs::create_dir_all(par)?;
            }
            let mut o = std::fs::File::create(&p)?;
            std::io::copy(&mut e, &mut o)?;
            #[cfg(unix)]
            if let Some(mode) = e.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))?;
            }
            out.push(p);
        }
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".tar") {
        let f = std::fs::File::open(archive)?;
        let rd: Box<dyn Read> = if name.ends_with(".tar") {
            Box::new(f)
        } else {
            Box::new(flate2::read::GzDecoder::new(f))
        };
        let mut t = tar::Archive::new(rd);
        for e in t.entries()? {
            let mut e = e?;
            let rel = e.path()?.to_path_buf();
            if rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                continue;
            }
            let p = dest.join(&rel);
            e.unpack_in(dest)?;
            if p.is_file() {
                out.push(p);
            }
        }
    } else {
        bail!("unsupported archive format: {}", archive.display());
    }
    Ok(out)
}

/// Mark a file executable (no-op on Windows).
pub fn make_executable(p: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(p)?.permissions();
        perm.set_mode(perm.mode() | 0o755);
        std::fs::set_permissions(p, perm)?;
    }
    #[cfg(not(unix))]
    let _ = p;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_parse() {
        let h = "a".repeat(64);
        assert_eq!(
            parse_sha256_sidecar(&format!("{h}  ffmpeg.zip\n")),
            Some(h.clone())
        );
        assert_eq!(parse_sha256_sidecar(&h.to_uppercase()), Some(h));
        assert_eq!(parse_sha256_sidecar("nope"), None);
    }

    #[test]
    fn zip_extract_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let zp = dir.path().join("a.zip");
        {
            let f = std::fs::File::create(&zp).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let o: zip::write::SimpleFileOptions = Default::default();
            w.start_file("sub/ffmpeg", o).unwrap();
            w.write_all(b"hello").unwrap();
            w.finish().unwrap();
        }
        let files = extract(&zp, &dir.path().join("x")).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(std::fs::read(&files[0]).unwrap(), b"hello");
        assert_eq!(sha256_file(&zp).unwrap().len(), 64);
    }
}
