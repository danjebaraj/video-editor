//! The `claude -p --output-format stream-json` protocol: parsing output lines and building the
//! lines the host writes to stdin. Ported from Just Notes' `StreamEvents.swift`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// Total cost reported by the CLI in the result event.
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub num_turns: Option<u64>,
}

impl Usage {
    pub fn total_input(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens
    }
    fn from_json(u: &Value) -> Usage {
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        Usage {
            input_tokens: n("input_tokens"),
            output_tokens: n("output_tokens"),
            cache_read_tokens: n("cache_read_input_tokens"),
            cache_creation_tokens: n("cache_creation_input_tokens"),
            ..Usage::default()
        }
    }
}

/// A question asked through Claude's AskUserQuestion tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Question {
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Init {
        session_id: String,
        model: Option<String>,
        tools: Vec<String>,
        cli_version: Option<String>,
    },
    /// Incremental text of the main conversation (needs `--include-partial-messages`).
    TextDelta(String),
    /// A complete assistant text block.
    AssistantText(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Token usage of one assistant message (`message_id` repeats across its content blocks).
    MessageUsage {
        message_id: String,
        usage: Usage,
    },
    /// The host must answer this `can_use_tool` control request.
    PermissionRequest {
        request_id: String,
        tool_name: String,
        input: Value,
        tool_use_id: Option<String>,
    },
    Result {
        is_error: bool,
        text: Option<String>,
        session_id: Option<String>,
        subtype: Option<String>,
        usage: Option<Usage>,
    },
    ControlResponse {
        request_id: String,
    },
    Ignored(String),
}

fn is_main_thread(obj: &Value) -> bool {
    obj.get("parent_tool_use_id").map(Value::is_null).unwrap_or(true)
}

/// Parses one output line into zero or more events. Lines that aren't JSON yield nothing.
pub fn parse_line(line: &str) -> Vec<StreamEvent> {
    let Ok(obj) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let Some(kind) = obj["type"].as_str() else { return Vec::new() };
    match kind {
        "system" => {
            if obj["subtype"] == "init" {
                if let Some(sid) = obj["session_id"].as_str() {
                    let tools = obj["tools"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    return vec![StreamEvent::Init {
                        session_id: sid.to_string(),
                        model: obj["model"].as_str().map(str::to_string),
                        tools,
                        cli_version: obj["claude_code_version"].as_str().map(str::to_string),
                    }];
                }
            }
            vec![StreamEvent::Ignored("system".into())]
        }
        "stream_event" => {
            let ev = &obj["event"];
            if ev["type"] == "content_block_delta" && ev["delta"]["type"] == "text_delta" && is_main_thread(&obj) {
                if let Some(t) = ev["delta"]["text"].as_str() {
                    return vec![StreamEvent::TextDelta(t.to_string())];
                }
            }
            vec![StreamEvent::Ignored("stream_event".into())]
        }
        "assistant" => {
            let msg = &obj["message"];
            let mut out = Vec::new();
            if let (Some(id), true) = (msg["id"].as_str(), msg["usage"].is_object()) {
                out.push(StreamEvent::MessageUsage {
                    message_id: id.to_string(),
                    usage: Usage::from_json(&msg["usage"]),
                });
            }
            let main = is_main_thread(&obj);
            for block in msg["content"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                match block["type"].as_str() {
                    Some("text") if main => {
                        if let Some(t) = block["text"].as_str().filter(|t| !t.is_empty()) {
                            out.push(StreamEvent::AssistantText(t.to_string()));
                        }
                    }
                    Some("tool_use") => out.push(StreamEvent::ToolUse {
                        id: block["id"].as_str().unwrap_or("").to_string(),
                        name: block["name"].as_str().unwrap_or("").to_string(),
                        input: block["input"].clone(),
                    }),
                    _ => {}
                }
            }
            out
        }
        "user" => obj["message"]["content"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|b| b["type"] == "tool_result")
                    .map(|b| {
                        let content = match &b["content"] {
                            Value::String(s) => s.clone(),
                            Value::Array(parts) => {
                                parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")
                            }
                            _ => String::new(),
                        };
                        StreamEvent::ToolResult {
                            tool_use_id: b["tool_use_id"].as_str().unwrap_or("").to_string(),
                            content,
                            is_error: b["is_error"].as_bool().unwrap_or(false),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        "control_request" => {
            let req = &obj["request"];
            match (obj["request_id"].as_str(), req["subtype"].as_str()) {
                (Some(rid), Some("can_use_tool")) => vec![StreamEvent::PermissionRequest {
                    request_id: rid.to_string(),
                    tool_name: req["tool_name"].as_str().unwrap_or("").to_string(),
                    input: req["input"].clone(),
                    tool_use_id: req["tool_use_id"].as_str().map(str::to_string),
                }],
                _ => vec![StreamEvent::Ignored("control_request".into())],
            }
        }
        "control_response" => vec![StreamEvent::ControlResponse {
            request_id: obj["response"]["request_id"].as_str().unwrap_or("").to_string(),
        }],
        "result" => {
            let usage = obj["usage"].is_object().then(|| {
                let mut u = Usage::from_json(&obj["usage"]);
                // `usage` covers the last turn only; `modelUsage` holds the totals for the
                // whole session, per model.
                if let Some(models) = obj["modelUsage"].as_object().filter(|m| !m.is_empty()) {
                    let sum = |k: &str| models.values().map(|m| m[k].as_u64().unwrap_or(0)).sum::<u64>();
                    u.input_tokens = sum("inputTokens");
                    u.output_tokens = sum("outputTokens");
                    u.cache_read_tokens = sum("cacheReadInputTokens");
                    u.cache_creation_tokens = sum("cacheCreationInputTokens");
                }
                u.cost_usd = obj["total_cost_usd"].as_f64();
                u.duration_ms = obj["duration_ms"].as_u64();
                u.num_turns = obj["num_turns"].as_u64();
                u
            });
            vec![StreamEvent::Result {
                is_error: obj["is_error"].as_bool().unwrap_or(false),
                text: obj["result"].as_str().map(str::to_string),
                session_id: obj["session_id"].as_str().map(str::to_string),
                subtype: obj["subtype"].as_str().map(str::to_string),
                usage,
            }]
        }
        other => vec![StreamEvent::Ignored(other.to_string())],
    }
}

/// The questions in an AskUserQuestion tool input.
pub fn questions(input: &Value) -> Vec<Question> {
    input["questions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|q| {
                    Some(Question {
                        question: q["question"].as_str()?.to_string(),
                        header: q["header"].as_str().map(str::to_string),
                        options: q["options"]
                            .as_array()
                            .map(|o| {
                                o.iter()
                                    .filter_map(|x| {
                                        Some(QuestionOption {
                                            label: x["label"].as_str()?.to_string(),
                                            description: x["description"].as_str().map(str::to_string),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        multi_select: q["multiSelect"].as_bool().unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// MARK: Lines written to the CLI's stdin

pub fn user_message(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": text}}).to_string()
}

pub fn initialize_request(id: &str) -> String {
    json!({"type": "control_request", "request_id": id, "request": {"subtype": "initialize"}}).to_string()
}

pub fn interrupt_request(id: &str) -> String {
    json!({"type": "control_request", "request_id": id, "request": {"subtype": "interrupt"}}).to_string()
}

pub fn allow(request_id: &str, input: &Value) -> String {
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"behavior": "allow", "updatedInput": input}}}).to_string()
}

pub fn deny(request_id: &str, message: &str) -> String {
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"behavior": "deny", "message": message}}}).to_string()
}

/// AskUserQuestion input with the answers filled in (question text → answer, labels comma-joined).
pub fn answered_input(input: &Value, answers: &std::collections::BTreeMap<String, String>) -> Value {
    let mut out = input.clone();
    if let Some(o) = out.as_object_mut() {
        o.insert("answers".into(), json!(answers));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init_text_tools_and_result() {
        let init = parse_line(
            r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-haiku-5-5","tools":["Read","Bash"],"claude_code_version":"2.1.311"}"#,
        );
        assert_eq!(
            init,
            vec![StreamEvent::Init {
                session_id: "s1".into(),
                model: Some("claude-haiku-5-5".into()),
                tools: vec!["Read".into(), "Bash".into()],
                cli_version: Some("2.1.311".into())
            }]
        );
        let delta = parse_line(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}},"parent_tool_use_id":null}"#,
        );
        assert_eq!(delta, vec![StreamEvent::TextDelta("Hi".into())]);
        let sub = parse_line(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"x"}},"parent_tool_use_id":"t9"}"#,
        );
        assert_eq!(sub, vec![StreamEvent::Ignored("stream_event".into())], "subagent text isn't shown");
        let asst = parse_line(
            r#"{"type":"assistant","message":{"id":"m1","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100},"content":[{"type":"text","text":"Done"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cutline validate project.json5"}}]}}"#,
        );
        assert_eq!(asst.len(), 3);
        assert!(
            matches!(&asst[0], StreamEvent::MessageUsage { message_id, usage } if message_id == "m1" && usage.total_input() == 110 && usage.output_tokens == 5)
        );
        assert_eq!(asst[1], StreamEvent::AssistantText("Done".into()));
        assert!(
            matches!(&asst[2], StreamEvent::ToolUse { name, input, .. } if name == "Bash" && input["command"] == "cutline validate project.json5")
        );
        let res = parse_line(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"ok"},{"type":"text","text":"2"}],"is_error":true}]}}"#,
        );
        assert_eq!(
            res,
            vec![StreamEvent::ToolResult { tool_use_id: "t1".into(), content: "ok\n2".into(), is_error: true }]
        );
        let done = parse_line(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"fin","session_id":"s1","total_cost_usd":0.25,"duration_ms":1200,"num_turns":4,"usage":{"input_tokens":1,"output_tokens":2}}"#,
        );
        assert!(
            matches!(&done[0], StreamEvent::Result { is_error: false, usage: Some(u), .. } if u.cost_usd == Some(0.25) && u.num_turns == Some(4))
        );
        // the session totals, not the last turn's usage
        let done = parse_line(
            r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":11.0,"usage":{"input_tokens":18,"output_tokens":4895},"modelUsage":{"claude-opus-5-5":{"inputTokens":362,"outputTokens":125303,"cacheReadInputTokens":29206706,"cacheCreationInputTokens":333858}}}"#,
        );
        assert!(
            matches!(&done[0], StreamEvent::Result { usage: Some(u), .. } if u.output_tokens == 125_303 && u.total_input() == 362 + 29_206_706 + 333_858)
        );
        assert!(parse_line("not json").is_empty());
    }

    #[test]
    fn permission_requests_and_responses() {
        let req = parse_line(
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Tone?","header":"Tone","multiSelect":true,"options":[{"label":"Calm","description":"Soft"},{"label":"Bold"}]}]},"tool_use_id":"t2"}}"#,
        );
        let StreamEvent::PermissionRequest { request_id, tool_name, input, tool_use_id } = &req[0] else { panic!() };
        assert_eq!(
            (request_id.as_str(), tool_name.as_str(), tool_use_id.as_deref()),
            ("r1", "AskUserQuestion", Some("t2"))
        );
        let qs = questions(input);
        assert_eq!(qs.len(), 1);
        assert!(
            qs[0].multi_select && qs[0].options.len() == 2 && qs[0].options[0].description.as_deref() == Some("Soft")
        );
        let mut answers = std::collections::BTreeMap::new();
        answers.insert("Tone?".to_string(), "Calm, Bold".to_string());
        let line = allow("r1", &answered_input(input, &answers));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["response"]["response"]["behavior"], "allow");
        assert_eq!(v["response"]["response"]["updatedInput"]["answers"]["Tone?"], "Calm, Bold");
        let d: Value = serde_json::from_str(&deny("r1", "no")).unwrap();
        assert_eq!(
            (d["response"]["request_id"].as_str(), d["response"]["response"]["message"].as_str()),
            (Some("r1"), Some("no"))
        );
        assert_eq!(
            parse_line(r#"{"type":"control_response","response":{"request_id":"app-1","subtype":"success"}}"#),
            vec![StreamEvent::ControlResponse { request_id: "app-1".into() }]
        );
    }
}
