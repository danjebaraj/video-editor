//! The messages between the web UI and the shell (ui/spec.md §23.3, "Bridge").
//!
//! Every message is JSON through `postMessage` / `WebMessageReceived`: a request with an id, the
//! response to it, or an event. These Rust types are the source of truth; `ui/src/generated/`
//! holds their JSON Schema (written by the `bridge_schema_is_current` test) and the TypeScript
//! generated from it at build time, so the two sides can't drift. Core data inside the messages
//! (projects, reports, run events) is the core's own JSON, typed from `schema/cutproj.schema.json`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// UI → shell.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub id: u64,
    pub method: Method,
    #[serde(default)]
    pub args: Value,
}

/// Shell → UI.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostMessage {
    Response {
        id: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<HostError>,
    },
    Event {
        topic: Topic,
        data: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct HostError {
    /// The core's stable error codes (`not_found`, `environment`, `bad_request`, …) or `shell`.
    pub code: String,
    pub message: String,
}

/// Everything the UI can ask of the shell.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
pub enum Method {
    /// One core operation, exactly as the macOS shell calls it: [`CoreArgs`].
    #[serde(rename = "core")]
    Core,
    /// Stores a pasted image in the project's `.assets` sidecar: [`PastedImageArgs`] → path.
    #[serde(rename = "project.store_pasted_image")]
    StorePastedImage,
    /// [`RunStartArgs`] → [`RunStarted`]. Events arrive on the `run` topic.
    #[serde(rename = "run.start")]
    RunStart,
    #[serde(rename = "run.cancel")]
    RunCancel,
    #[serde(rename = "run.kill")]
    RunKill,
    #[serde(rename = "run.answer")]
    RunAnswer,
    /// [`AssistantStartArgs`] → `{token}`. Events arrive on the `assistant` topic.
    #[serde(rename = "assistant.start")]
    AssistantStart,
    #[serde(rename = "assistant.stop")]
    AssistantStop,
    #[serde(rename = "assistant.answer")]
    AssistantAnswer,
    #[serde(rename = "assistant.dismiss")]
    AssistantDismiss,
    /// [`OpenDialogArgs`] → `string[]` (empty when cancelled).
    #[serde(rename = "dialog.open")]
    DialogOpen,
    /// [`SaveDialogArgs`] → `string | null`.
    #[serde(rename = "dialog.save")]
    DialogSave,
    /// [`ConfirmArgs`] → index of the button chosen.
    #[serde(rename = "dialog.confirm")]
    DialogConfirm,
    #[serde(rename = "shell.info")]
    Info,
    #[serde(rename = "shell.reveal")]
    Reveal,
    #[serde(rename = "shell.open")]
    Open,
    #[serde(rename = "shell.open_url")]
    OpenUrl,
    #[serde(rename = "shell.notify")]
    Notify,
    /// [`ProgressArgs`]: taskbar progress for this window.
    #[serde(rename = "shell.progress")]
    Progress,
    #[serde(rename = "shell.set_title")]
    SetTitle,
    #[serde(rename = "shell.set_dirty")]
    SetDirty,
    /// [`OpenWindowArgs`]: a project, the App Library, Settings or the start window.
    #[serde(rename = "shell.open_window")]
    OpenWindow,
    /// Closes this window (the UI has saved first).
    #[serde(rename = "shell.close_window")]
    CloseWindow,
    #[serde(rename = "shell.quit")]
    Quit,
    #[serde(rename = "shell.recent")]
    Recent,
    #[serde(rename = "shell.add_recent")]
    AddRecent,
    /// [`PathArgs`]: report changes to this file on the `file_changed` topic.
    #[serde(rename = "shell.watch")]
    Watch,
    /// [`AllowMediaArgs`]: the files this window may load through the media host.
    #[serde(rename = "shell.allow_media")]
    AllowMedia,
    /// [`PathArgs`] → URL of an allowed media file on the media host.
    #[serde(rename = "shell.media_url")]
    MediaUrl,
    /// [`SecretArgs`]: the ElevenLabs key, through the bundled helper (never read by the app).
    #[serde(rename = "secret.status")]
    SecretStatus,
    #[serde(rename = "secret.set")]
    SecretSet,
    #[serde(rename = "secret.test")]
    SecretTest,
    #[serde(rename = "secret.delete")]
    SecretDelete,
    /// [`WriteTextArgs`]: writes a text file the user just chose in a Save dialog (Export Spec).
    #[serde(rename = "shell.write_text")]
    WriteText,
    /// [`RecycleArgs`] → `{moved}`: moves pasted images from a `.cutproj.assets` sidecar to the
    /// Recycle Bin (Clean Up Pasted Images). Nothing else can be deleted this way.
    #[serde(rename = "shell.recycle")]
    Recycle,
    /// [`SessionsArgs`] → the project's saved assistant transcripts (`[]` when none).
    #[serde(rename = "assistant.load_sessions")]
    LoadSessions,
    /// [`SessionsArgs`] with `sessions`: writes them.
    #[serde(rename = "assistant.save_sessions")]
    SaveSessions,
    /// [`SessionsArgs`] with `session_id`: removes that session's scratch folder.
    #[serde(rename = "assistant.delete_session")]
    DeleteSession,
    /// [`CutlineArgs`] → `{code, output}`: `cutline doctor` or `cutline deps install --all`.
    #[serde(rename = "shell.cutline")]
    Cutline,
    /// [`PathArgs`]: saves a PNG of this window's content (self test, UI tests).
    #[serde(rename = "shell.snapshot")]
    Snapshot,
    /// [`SelfTestReport`]: a self-test step finished (self test only).
    #[serde(rename = "selftest.report")]
    SelfTestReport,
    /// [`SelfTestFsArgs`]: renames, copies or removes a folder inside the self test's scratch
    /// folder (the relink step). Refused outside the self test.
    #[serde(rename = "selftest.fs")]
    SelfTestFs,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// A run event (`events.ndjson` schema), with the run's `token`: [`RunEvent`].
    Run,
    /// An assistant event, with the turn's `token`.
    Assistant,
    /// A menu command or accelerator: `{command}`.
    Menu,
    /// The system theme, contrast, accent or DPI changed: [`ThemeInfo`].
    Theme,
    /// A watched file changed on disk: `{path}`.
    FileChanged,
    /// Files to open (double-click, jump list, a second instance): `{paths}`.
    OpenFiles,
    /// The user asked to close the window: save, then `shell.close_window`.
    CloseRequested,
    /// The self test asks the UI to perform a step: `{step, args}`.
    SelfTest,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CoreArgs {
    pub method: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PastedImageArgs {
    pub project_path: String,
    /// The image bytes (PNG, JPEG, …), base64.
    pub image_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunStartArgs {
    /// A core `RunRequest`.
    pub request: Value,
    /// Options only the shell knows are filled in by it (`helper`, `eleven_key_present`).
    #[serde(default)]
    pub options: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunStarted {
    pub token: u64,
    pub run_id: String,
    pub folder: String,
}

/// Payload of a `run` event: one core event, or the run's end with its outcome.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunEvent {
    pub token: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TokenArgs {
    pub token: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AnswerArgs {
    pub token: u64,
    pub question_id: String,
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssistantStartArgs {
    /// A core `AssistantRequest`.
    pub request: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileFilter {
    pub name: String,
    /// `*.cutproj;*.json`
    pub spec: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct OpenDialogArgs {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub filters: Vec<FileFilter>,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub folders: bool,
    #[serde(default)]
    pub default_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SaveDialogArgs {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub filters: Vec<FileFilter>,
    #[serde(default)]
    pub default_name: Option<String>,
    #[serde(default)]
    pub default_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ConfirmArgs {
    pub title: String,
    pub message: String,
    /// Button labels, the default first.
    pub buttons: Vec<String>,
    #[serde(default)]
    pub warning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PathArgs {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenArgs {
    pub path: String,
    /// `None`: the default app. `""`: the Open With dialog. Otherwise a program to open it with.
    #[serde(default)]
    pub application: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UrlArgs {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotifyArgs {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProgressState {
    None,
    Normal,
    Indeterminate,
    Error,
    Paused,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProgressArgs {
    pub state: ProgressState,
    /// 0…1
    #[serde(default)]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TitleArgs {
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DirtyArgs {
    pub dirty: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Start,
    Project,
    Library,
    Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenWindowArgs {
    pub kind: WindowKind,
    /// The project file, for a project window. Omitted: an untitled project.
    #[serde(default)]
    pub path: Option<String>,
    /// For an untitled project: what to make (`{name, intent, template}`).
    #[serde(default)]
    pub options: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AllowMediaArgs {
    /// Absolute paths of the project's resolved media files.
    pub files: Vec<String>,
    /// The project's `.assets` sidecar folder, whose images are allowed too.
    #[serde(default)]
    pub sidecar: Option<String>,
    /// Add to what is allowed (run outputs, logs) instead of replacing the project's list.
    #[serde(default)]
    pub add: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SecretArgs {
    /// For `secret.set`: the key.
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SelfTestFsArgs {
    /// `rename`, `copy` or `remove`.
    pub op: String,
    pub from: String,
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteTextArgs {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecycleArgs {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionsArgs {
    pub project_id: String,
    #[serde(default)]
    pub sessions: Option<Value>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CutlineAction {
    Doctor,
    InstallDeps,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CutlineArgs {
    pub action: CutlineAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SelfTestReport {
    pub step: String,
    pub ok: bool,
    #[serde(default)]
    pub detail: String,
}

/// `shell.info`: what the UI needs to know about the machine and the shell.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ShellInfo {
    pub app_version: String,
    pub core_version: String,
    pub platform: String,
    pub arch: String,
    pub window_kind: WindowKind,
    pub window_id: u64,
    pub helper: String,
    pub webview_version: String,
    pub debug: bool,
    pub selftest: bool,
    pub theme: ThemeInfo,
    /// Files passed on the command line for this window.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub options: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ThemeInfo {
    pub dark: bool,
    pub high_contrast: bool,
    /// `#rrggbb`
    pub accent: String,
    /// Scale factor of the window's monitor (1.0 = 96 DPI).
    pub scale: f64,
    /// The Mica backdrop shows through the page. False when transparency effects are off, in
    /// high contrast, and for snapshots: the page then paints a solid background.
    #[serde(default)]
    pub mica: bool,
}

/// The JSON Schema of every bridge type, for the TypeScript generator.
pub fn schema() -> Value {
    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct Bridge {
        request: Request,
        host_message: HostMessage,
        core_args: CoreArgs,
        pasted_image_args: PastedImageArgs,
        run_start_args: RunStartArgs,
        run_started: RunStarted,
        run_event: RunEvent,
        token_args: TokenArgs,
        answer_args: AnswerArgs,
        assistant_start_args: AssistantStartArgs,
        open_dialog_args: OpenDialogArgs,
        save_dialog_args: SaveDialogArgs,
        confirm_args: ConfirmArgs,
        path_args: PathArgs,
        open_args: OpenArgs,
        url_args: UrlArgs,
        notify_args: NotifyArgs,
        progress_args: ProgressArgs,
        title_args: TitleArgs,
        dirty_args: DirtyArgs,
        open_window_args: OpenWindowArgs,
        allow_media_args: AllowMediaArgs,
        secret_args: SecretArgs,
        cutline_args: CutlineArgs,
        sessions_args: SessionsArgs,
        write_text_args: WriteTextArgs,
        self_test_fs_args: SelfTestFsArgs,
        recycle_args: RecycleArgs,
        self_test_report: SelfTestReport,
        shell_info: ShellInfo,
    }
    serde_json::to_value(schemars::schema_for!(Bridge)).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UI's copy of the bridge schema matches these types (regenerate with
    /// `VIDEO_STUDIO_BLESS=1 cargo test -p video-studio-win bridge_schema_is_current`).
    #[test]
    fn bridge_schema_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/ui/src/generated/bridge.schema.json");
        let text = serde_json::to_string_pretty(&schema()).unwrap() + "\n";
        if std::env::var_os("VIDEO_STUDIO_BLESS").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, &text).unwrap();
        }
        let committed = std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n");
        assert_eq!(committed, text, "ui/src/generated/bridge.schema.json is stale: run with VIDEO_STUDIO_BLESS=1");
    }

    #[test]
    fn messages_round_trip() {
        let r: Request = serde_json::from_str(r#"{"id":3,"method":"core","args":{"method":"library.dir"}}"#).unwrap();
        assert_eq!((r.id, r.method), (3, Method::Core));
        let m: Request = serde_json::from_str(r#"{"id":4,"method":"shell.reveal","args":{"path":"C:\\x"}}"#).unwrap();
        assert_eq!(m.method, Method::Reveal);
        let ev = HostMessage::Event { topic: Topic::FileChanged, data: serde_json::json!({"path": "x"}) };
        assert_eq!(serde_json::to_value(&ev).unwrap()["topic"], "file_changed");
        let resp = HostMessage::Response {
            id: 1,
            ok: false,
            result: None,
            error: Some(HostError { code: "shell".into(), message: "no".into() }),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!((v["kind"].as_str(), v["ok"].as_bool()), (Some("response"), Some(false)));
        assert!(serde_json::from_str::<Request>(r#"{"id":1,"method":"shell.format_c"}"#).is_err());
    }
}
