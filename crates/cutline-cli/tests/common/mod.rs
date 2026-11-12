#![allow(dead_code)]
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

pub fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cutline"))
}

pub fn run_json(cmd: &mut Command) -> (i32, serde_json::Value, String) {
    let out = cmd.arg("--json").output().expect("run cutline");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let v = serde_json::from_str(&stdout).unwrap_or(serde_json::Value::Null);
    (
        out.status.code().unwrap_or(-1),
        v,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Minimal static HTTP server for tests: serves files from a map path → bytes.
pub struct MockServer {
    pub base: String,
    pub files: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

impl MockServer {
    pub fn start() -> Self {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let files: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>> = Default::default();
        let f2 = files.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut s = s;
                let mut buf = [0u8; 8192];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = f2.lock().unwrap().get(&path).cloned();
                let resp = match body {
                    Some(b) => {
                        let mut r = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            b.len()
                        )
                        .into_bytes();
                        if !req.starts_with("HEAD") {
                            r.extend(b);
                        }
                        r
                    }
                    None => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                let _ = s.write_all(&resp);
            }
        });
        Self { base, files }
    }
    pub fn put(&self, path: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_string(), data);
    }
}

pub fn sha256_hex(b: &[u8]) -> String {
    // tiny dependency-free sha via the cutline binary is overkill; use std command `sha256sum` fallback
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("x");
    std::fs::write(&p, b).unwrap();
    let out = Command::new("sha256sum")
        .arg(&p)
        .output()
        .or_else(|_| Command::new("shasum").args(["-a", "256"]).arg(&p).output())
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

/// Zip containing a fake `ffmpeg`/`ffprobe` shell script reporting `version`.
pub fn fake_ffmpeg_zip(dir: &Path, name: &str, version: &str) -> Vec<u8> {
    let p: PathBuf = dir.join(format!("{name}-{version}.zip"));
    {
        let f = std::fs::File::create(&p).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let o: zip::write::SimpleFileOptions =
            zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        w.start_file(name, o).unwrap();
        w.write_all(format!("#!/bin/sh\necho \"{name} version {version} Copyright\"\n").as_bytes())
            .unwrap();
        w.finish().unwrap();
    }
    std::fs::read(&p).unwrap()
}
