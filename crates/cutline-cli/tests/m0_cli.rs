mod common;
use common::*;

#[test]
fn doctor_json_lists_capabilities() {
    let (code, v, err) = run_json(bin().arg("doctor"));
    assert!(code == 0 || code == 3, "{err}");
    let caps = v["capabilities"].as_array().expect("capabilities array");
    assert!(caps.iter().any(|c| c["name"] == "xfade" && c["kind"] == "filter"));
    assert!(caps.iter().any(|c| c["name"] == "libx264" && c["kind"] == "encoder"));
    assert!(v.get("fixes").is_some());
}

#[test]
fn usage_error_exit_code() {
    let out = bin().arg("no-such-command").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = bin().arg("probe").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn probe_missing_file_is_validation_error() {
    let (code, v, _) = run_json(bin().args(["probe", "/definitely/not/here.mp4"]));
    assert_eq!(code, 1);
    assert_eq!(v["ok"], false);
}

/// `deps install ffmpeg` (from a mock mirror) installs a working binary and records it in the
/// lockfile; `deps update` stages, selftests, promotes; `deps rollback` restores.
#[cfg(unix)]
#[test]
fn deps_install_update_rollback_with_mock_server() {
    let srv = MockServer::start();
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let lock = d.path().join("tools.lock.json");
    let publish = |ver: &str| {
        let a = fake_ffmpeg_zip(d.path(), "ffmpeg", ver);
        let b = fake_ffmpeg_zip(d.path(), "ffprobe", ver);
        let (sa, sb) = (sha256_hex(&a), sha256_hex(&b));
        srv.put(&format!("/{ver}/ffmpeg.zip"), a);
        srv.put(&format!("/{ver}/ffprobe.zip"), b);
        let m = serde_json::json!({"version": ver, "artifacts": [
            {"name": "ffmpeg", "url": format!("{}/{ver}/ffmpeg.zip", srv.base), "sha256": sa},
            {"name": "ffprobe", "url": format!("{}/{ver}/ffprobe.zip", srv.base), "sha256": sb}]});
        srv.put("/manifest.json", serde_json::to_vec(&m).unwrap());
    };
    let env = |c: &mut std::process::Command| {
        c.env("CUTLINE_HOME", &home)
            .env("CUTLINE_LOCKFILE", &lock)
            .env("CUTLINE_FFMPEG_MANIFEST_URL", format!("{}/manifest.json", srv.base))
            .env_remove("HTTPS_PROXY")
            .env_remove("https_proxy")
            .env("NO_PROXY", "127.0.0.1");
    };
    publish("7.1.1");
    let mut c = bin();
    env(&mut c);
    let (code, v, err) = run_json(c.args(["deps", "install", "ffmpeg"]));
    assert_eq!(code, 0, "{err} {v}");
    assert_eq!(v["version"], "7.1.1");
    let l: serde_json::Value = serde_json::from_slice(&std::fs::read(&lock).unwrap()).unwrap();
    assert_eq!(l["tools"].as_array().unwrap().len(), 2);
    let out = std::process::Command::new(home.join("tools/ffmpeg/7.1.1/ffmpeg")).arg("-version").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("7.1.1"));

    // sha mismatch is rejected
    srv.put(
        "/manifest.json",
        serde_json::to_vec(&serde_json::json!({"version": "bad", "artifacts": [
        {"name": "ffmpeg", "url": format!("{}/7.1.1/ffmpeg.zip", srv.base), "sha256": "0".repeat(64)},
        {"name": "ffprobe", "url": format!("{}/7.1.1/ffprobe.zip", srv.base), "sha256": "0".repeat(64)}]}))
        .unwrap(),
    );
    let mut c = bin();
    env(&mut c);
    let (code, _, _) = run_json(c.args(["deps", "install", "ffmpeg", "--latest"]));
    assert_eq!(code, 3);

    // update: new version whose selftest fails → rejected, current kept
    publish("8.0");
    let mut c = bin();
    env(&mut c);
    c.env("CUTLINE_UPDATE_SELFTEST_CMD", "false");
    let (code, v, _) = run_json(c.args(["deps", "update"]));
    assert_eq!(code, 3, "{v}");
    assert_eq!(v["updated"], false);
    assert_eq!(std::fs::read_to_string(home.join("tools/ffmpeg/current")).unwrap(), "7.1.1");

    // update: selftest passes → promoted
    let mut c = bin();
    env(&mut c);
    c.env("CUTLINE_UPDATE_SELFTEST_CMD", "true");
    let (code, v, err) = run_json(c.args(["deps", "update"]));
    assert_eq!(code, 0, "{err} {v}");
    assert_eq!(v["updated"], true);
    assert_eq!(std::fs::read_to_string(home.join("tools/ffmpeg/current")).unwrap(), "8.0");
    let l: serde_json::Value = serde_json::from_slice(&std::fs::read(&lock).unwrap()).unwrap();
    assert!(l["tools"].as_array().unwrap().iter().all(|t| t["version"] == "8.0" && t["selftest"] == "pass"));

    // rollback
    let mut c = bin();
    env(&mut c);
    let (code, v, _) = run_json(c.args(["deps", "rollback"]));
    assert_eq!(code, 0);
    assert_eq!(v["current"], "7.1.1");
    assert_eq!(std::fs::read_to_string(home.join("tools/ffmpeg/current")).unwrap(), "7.1.1");
}
