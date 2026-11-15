//! Files and inputs made on one platform work on the other (ui/spec.md §23.1, §23.5, §23.9):
//! CRLF input everywhere, LF output, each platform's file reference kept by the other, generated
//! names Windows can create, and (on Windows) a Credential Manager round trip.

use std::collections::HashSet;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};
use video_studio_core::media::{self, MediaKind, MediaStatus};
use video_studio_core::project::{self, Project};
use video_studio_core::{paths, spec};

fn tmp() -> (tempfile::TempDir, Utf8PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = paths::canonical_or_normal(Utf8Path::from_path(d.path()).unwrap());
    (d, p)
}

const SPEC_LF: &str = "# Launch\n\n## Scene 1: Hook\n- **Duration:** 5 s\n- **Media:** ![hero](media:m-hero)\n\n```vo\nMeet the fastest way to ship.\n```\n\n## Call to action\nStart a free trial. TODO pick the URL\n";

#[test]
fn a_crlf_spec_reads_like_the_lf_one() {
    let crlf = SPEC_LF.replace('\n', "\r\n");
    assert_eq!(spec::outline(&crlf), spec::outline(SPEC_LF));
    // byte offsets index the text as given; the references and their lines are the same
    let refs = |s: &str| spec::media_refs(s).into_iter().map(|r| (r.id, r.line)).collect::<Vec<_>>();
    assert_eq!(refs(&crlf), refs(SPEC_LF));
    assert_eq!(spec::voice_overs(&crlf), spec::voice_overs(SPEC_LF));
    assert_eq!(spec::placeholders(&crlf), spec::placeholders(SPEC_LF));
    assert_eq!(spec::has_call_to_action(&crlf), spec::has_call_to_action(SPEC_LF));
    let ids: HashSet<String> = HashSet::new();
    let ctx = spec::LintContext { media_ids: &ids, target_length_s: Some(3.0), words_per_minute: 150 };
    assert_eq!(spec::lint(&crlf, &ctx), spec::lint(SPEC_LF, &ctx));
    assert_eq!(spec::scenes(&crlf).len(), 1);
}

#[test]
fn a_crlf_project_file_loads_and_saves_with_lf() {
    let (_d, root) = tmp();
    let mut p = Project::new("Launch");
    p.spec = SPEC_LF.into();
    let lf = p.to_json().unwrap();
    // a project file that went through an editor or a Git checkout with CRLF line endings,
    // whose spec string has CRLF in it too
    let crlf = lf.replace("\\n", "\\r\\n").replace('\n', "\r\n");
    let path = root.join("launch.cutproj");
    std::fs::write(&path, &crlf).unwrap();
    let mut loaded = Project::load(&path).unwrap();
    assert_eq!(loaded.spec, SPEC_LF, "the spec is LF in memory");
    loaded.save(&path).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(!saved.contains('\r'), "written with LF");
    assert!(saved.ends_with("}\n"));
}

#[test]
fn a_crlf_scribe_transcript_and_brand_file_parse() {
    let scribe = "{\r\n  \"language_code\": \"en\",\r\n  \"text\": \"Hello there\",\r\n  \"words\": [\r\n    {\"text\": \"Hello\", \"start\": 0.0, \"end\": 0.4, \"type\": \"word\"}\r\n  ]\r\n}\r\n";
    let v: Value = serde_json::from_str(scribe).unwrap();
    assert_eq!(v["words"][0]["text"], "Hello");
    // the bundled brand presets read the same with CRLF
    let brand =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/brand/default.json")).unwrap();
    let a: Value = serde_json::from_str(&brand).unwrap();
    let b: Value = serde_json::from_str(&brand.replace('\n', "\r\n")).unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_project_made_on_macos_keeps_its_bookmark_when_saved_elsewhere() {
    let (_d, root) = tmp();
    std::fs::create_dir_all(root.join("footage")).unwrap();
    std::fs::write(root.join("footage/hero.mov"), b"hero").unwrap();
    // what the Mac app writes: a bookmark, a POSIX absolute path that doesn't exist here, and a
    // relative path that does
    let mac = json!({
        "format": "video-studio-project", "version": 1, "id": "prj_mac", "name": "From the Mac",
        "created": "2026-11-14T10:00:00Z", "modified": "2026-11-14T10:00:00Z",
        "intent": {"kind": "marketing"}, "spec": "# From the Mac\n",
        "media": [{"id": "m-hero", "kind": "video",
                   "paths": {"absolute": "/Users/ana/Footage/hero.mov", "relative": "footage/hero.mov",
                             "bookmark_macos": "Ym9va21hcmstZGF0YQ=="}}]
    });
    let path = root.join("from-mac.cutproj");
    std::fs::write(&path, serde_json::to_string_pretty(&mac).unwrap()).unwrap();
    let mut p = Project::load(&path).unwrap();
    let refs = video_studio_core::platform::system_file_refs();
    let res = media::resolve_all(&p, Some(&path), &[], refs.as_ref());
    assert_eq!(res[0].status, MediaStatus::Moved, "found by its relative path: {res:?}");
    media::apply_resolutions(&mut p, Some(&path), &res, refs.as_ref());
    p.save(&path).unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["media"][0]["paths"]["bookmark_macos"], "Ym9va21hcmstZGF0YQ==", "the Mac's bookmark is kept");
    // and the other way round: a Windows file ID survives a save on any platform
    p.media[0].paths.file_id_windows = Some("000000001234abcd:deadbeef000102030405060708090aff".into());
    p.save(&path).unwrap();
    let again = Project::load(&path).unwrap();
    assert_eq!(
        again.media[0].paths.file_id_windows.as_deref(),
        Some("000000001234abcd:deadbeef000102030405060708090aff")
    );
    assert_eq!(again.media[0].paths.bookmark_macos.as_deref(), Some("Ym9va21hcmstZGF0YQ=="));
}

#[cfg(windows)]
#[test]
fn media_added_on_windows_gets_a_file_id_and_follows_a_rename() {
    let (_d, root) = tmp();
    std::fs::create_dir_all(root.join("footage")).unwrap();
    std::fs::write(root.join("footage/hero.mov"), b"hero bytes").unwrap();
    let path = root.join("proj/p.cutproj");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut p = Project::new("P");
    let refs = video_studio_core::platform::system_file_refs();
    media::add_media(&mut p, Some(&path), &root.join("footage/hero.mov"), refs.as_ref()).unwrap();
    assert!(p.media[0].paths.file_id_windows.is_some() && p.media[0].paths.bookmark_macos.is_none());
    // renamed and moved: neither path works any more, the file ID still does
    std::fs::create_dir_all(root.join("archive")).unwrap();
    std::fs::rename(root.join("footage/hero.mov"), root.join("archive/hero-final.mov")).unwrap();
    let res = media::resolve_all(&p, Some(&path), &[], refs.as_ref());
    assert_eq!((res[0].status, res[0].found_by), (MediaStatus::Moved, Some(media::FoundBy::Bookmark)));
    assert!(res[0].path.as_deref().unwrap().ends_with("hero-final.mov"));
    // deleted: falls back to the search roots by fingerprint
    std::fs::create_dir_all(root.join("other-drive")).unwrap();
    std::fs::copy(root.join("archive/hero-final.mov"), root.join("other-drive/hero.mov")).unwrap();
    std::fs::remove_file(root.join("archive/hero-final.mov")).unwrap();
    let roots = vec![root.join("other-drive").to_string()];
    let res = media::resolve_all(&p, Some(&path), &roots, refs.as_ref());
    assert_eq!((res[0].status, res[0].found_by), (MediaStatus::Moved, Some(media::FoundBy::SearchRoot)));
}

#[test]
fn generated_names_avoid_what_windows_cant_create() {
    for (input, want) in [
        ("CON", "CON-"),
        ("nul", "nul-"),
        ("Com1", "Com1-"),
        ("LPT9.mp4", "LPT9.mp4-"),
        ("Console", "Console"),
        ("COM10", "COM10"),
        ("Launch: v2?", "Launch- v2-"),
        ("trailing dots...", "trailing dots"),
        ("  spaced  ", "spaced"),
    ] {
        assert_eq!(project::safe_file_name(input), want, "{input}");
    }
    assert!(project::is_reserved_windows_name("aux.txt") && !project::is_reserved_windows_name("auxiliary"));
}

#[test]
fn media_kinds_are_known_regardless_of_case() {
    for name in ["clip.MOV", "clip.mp4", "Track.WAV", "logo.PNG"] {
        assert_ne!(media::kind_for_path(Utf8Path::new(name)), MediaKind::Other, "{name}");
    }
}

/// Credential Manager round trip with the spec's service and account names. Windows only; it
/// needs a logged-on user session, and says so instead of failing when there is none.
#[cfg(windows)]
#[test]
fn credential_manager_round_trip() {
    use video_studio_core::platform::{OsSecrets, Secrets};
    let name = format!("vs-test-{}", std::process::id());
    match OsSecrets.set(&name, "s3cret-value") {
        Ok(()) => {}
        Err(e) => {
            eprintln!("skipped: Credential Manager isn't available in this session ({e})");
            return;
        }
    }
    assert_eq!(OsSecrets.get(&name).unwrap().as_deref(), Some("s3cret-value"));
    OsSecrets.set(&name, "changed").unwrap();
    assert_eq!(OsSecrets.get(&name).unwrap().as_deref(), Some("changed"));
    OsSecrets.delete(&name).unwrap();
    assert_eq!(OsSecrets.get(&name).unwrap(), None);
    OsSecrets.delete(&name).unwrap();
}
