//! A stand-in for `codex app-server` (FEAT-spry-alpaca-5RQZ tests): one JSON-RPC message per
//! line on stdio, with codex-cli 0.171.0's shapes. Behaviour from the environment:
//! - `FAKE_CODEX_LOG`: append every message received, one per line.
//! - `FAKE_CODEX_SIGNED_OUT=1`: `account/read` reports no account.
//! - `FAKE_CODEX_OLD=1`: answers every request with "method not found" (an incompatible build).
//! Before answering `account/read` it sends a notification and a server request of its own, so
//! clients must keep notifications and answer server requests with the request's ID.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

fn main() {
    let log = std::env::var("FAKE_CODEX_LOG").ok();
    let signed_out = std::env::var("FAKE_CODEX_SIGNED_OUT").is_ok();
    let old = std::env::var("FAKE_CODEX_OLD").is_ok();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut send = |v: Value| {
        let _ = writeln!(out, "{v}");
        let _ = out.flush();
    };
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if let Some(l) = &log {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(l) {
                let _ = writeln!(f, "{line}");
            }
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let (Some(id), Some(method)) = (msg.get("id").cloned(), msg["method"].as_str()) else { continue };
        if old {
            send(json!({"id": id, "error": {"code": -32601, "message": format!("unknown method {method}")}}));
            continue;
        }
        match method {
            "initialize" => send(
                json!({"id": id, "result": {"userAgent": "fake/0.171.0", "codexHome": "/tmp/fake-codex", "platformFamily": "unix", "platformOs": "fake"}}),
            ),
            "account/read" => {
                send(json!({"method": "account/updated", "params": {"authMode": "chatgpt"}}));
                send(json!({"id": "srv-1", "method": "item/tool/requestUserInput", "params": {"questions": []}}));
                if signed_out {
                    send(json!({"id": id, "result": {"account": null, "requiresOpenaiAuth": true}}));
                } else {
                    send(
                        json!({"id": id, "result": {"account": {"type": "chatgpt", "planType": "plus"}, "requiresOpenaiAuth": true}}),
                    );
                }
            }
            "model/list" => send(
                json!({"id": id, "result": {"data": [{"id": "gpt-5.5-codex", "displayName": "GPT-5.5 Codex", "isDefault": true}, {"id": "gpt-5.5-mini", "displayName": "GPT-5.5 mini", "isDefault": false}]}}),
            ),
            other => send(json!({"id": id, "error": {"code": -32601, "message": format!("unknown method {other}")}})),
        }
    }
}
