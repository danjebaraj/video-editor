//! FEAT-spry-alpaca-5RQZ: the Codex App Server transport and readiness check against a fake
//! app server (`vs-fake-codex`).

use camino::Utf8PathBuf;
use serde_json::{json, Value};
use std::time::Duration;
use video_studio_core::codex::{check, AppServer, CodexStatus};

const FAKE: &str = env!("CARGO_BIN_EXE_vs-fake-codex");
/// The fake reads its behaviour from the environment, which the children inherit: one test at
/// a time.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn handshake_account_and_models() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("codex.ndjson");
    std::env::set_var("FAKE_CODEX_LOG", &log);
    let mut s = AppServer::start(&Utf8PathBuf::from(FAKE)).unwrap();
    s.initialize().unwrap();
    let account = s.request("account/read", json!({}), Duration::from_secs(10)).unwrap();
    assert_eq!(account["account"]["planType"], "plus");
    // the notification was kept and the server's own request refused with its string ID
    assert!(s.notifications.iter().any(|n| n["method"] == "account/updated"));
    assert_eq!(s.refused_requests, vec!["item/tool/requestUserInput".to_string()]);
    // one more round trip: the fake has read (and logged) everything sent before it
    let models = s.request("model/list", json!({}), Duration::from_secs(10)).unwrap();
    assert_eq!(models["data"][0]["id"], "gpt-5.5-codex");
    drop(s);
    let sent: Vec<Value> =
        std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let methods: Vec<&str> = sent.iter().filter_map(|m| m["method"].as_str()).collect();
    assert_eq!(&methods[..3], ["initialize", "initialized", "account/read"], "initialization comes first");
    let answer = sent.iter().find(|m| m["id"] == "srv-1").expect("the server request was answered");
    assert_eq!(answer["error"]["code"], -32601);
    std::env::remove_var("FAKE_CODEX_LOG");

    match check(FAKE) {
        CodexStatus::Ready { plan, models } => {
            assert_eq!(plan.as_deref(), Some("plus"));
            assert_eq!(models.len(), 2);
            assert!(models[0].is_default && models[0].id == "gpt-5.5-codex");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn signed_out_incompatible_and_missing_are_distinct() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("FAKE_CODEX_SIGNED_OUT", "1");
    assert_eq!(check(FAKE), CodexStatus::SignInRequired);
    std::env::remove_var("FAKE_CODEX_SIGNED_OUT");
    std::env::set_var("FAKE_CODEX_OLD", "1");
    let old = check(FAKE);
    std::env::remove_var("FAKE_CODEX_OLD");
    assert!(matches!(&old, CodexStatus::Incompatible { detail } if detail.contains("initialize")), "{old:?}");
    assert_eq!(check("/no/such/codex"), CodexStatus::Missing);
    for (s, rule) in [
        (CodexStatus::Missing, "codex_missing"),
        (old, "codex_incompatible"),
        (CodexStatus::SignInRequired, "codex_signed_out"),
    ] {
        let (r, _, fix) = s.finding().unwrap();
        assert_eq!(r, rule);
        assert!(!fix.is_empty());
    }
}
