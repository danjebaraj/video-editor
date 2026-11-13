//! validate / schema / init / expand / render / preview / qa.

use crate::out::*;
use cutline_core::diag::{has_errors, Diagnostic};
use cutline_core::time::format_tc;
use cutline_render::pipeline::{self, LoadError, Loaded};
use cutline_render::RenderOptions;
use serde_json::json;
use std::path::{Path, PathBuf};

pub fn load(path: &Path) -> Result<Loaded, CliError> {
    pipeline::load(path).map_err(|e| match e {
        LoadError::Invalid(d) => CliError {
            code: EXIT_VALIDATION,
            kind: "validation",
            err: anyhow::anyhow!(serde_json::to_string(&d).unwrap_or_default()),
        },
        LoadError::Io(e) => CliError::usage(e),
        LoadError::Tool(e) => CliError::tool(e),
    })
}

pub fn diag_output(d: &[Diagnostic]) -> Output {
    let errors: Vec<&Diagnostic> = d.iter().filter(|x| x.is_error()).collect();
    let warnings: Vec<&Diagnostic> = d.iter().filter(|x| !x.is_error()).collect();
    let ok = errors.is_empty();
    let mut h = vec![];
    for x in d {
        h.push(format!(
            "{} {} {}: {}{}",
            if x.is_error() { "✘" } else { "⚠" },
            x.code,
            if x.path.is_empty() { "/" } else { &x.path },
            x.message,
            x.hint.as_deref().map(|h| format!("\n    hint: {h}")).unwrap_or_default()
        ));
    }
    if ok {
        h.push(format!("✔ valid ({} warning{})", warnings.len(), if warnings.len() == 1 { "" } else { "s" }));
    }
    Output::ok(json!({"ok": ok, "errors": errors, "warnings": warnings}), h.join("\n")).with_code(if ok {
        EXIT_OK
    } else {
        EXIT_VALIDATION
    })
}

/// Convert a load failure into diagnostics output where possible.
pub fn or_diag(r: Result<Loaded, CliError>) -> Result<Loaded, Result<Output, CliError>> {
    match r {
        Ok(l) => Ok(l),
        Err(e) if e.kind == "validation" => match serde_json::from_str::<Vec<Diagnostic>>(&e.err.to_string()) {
            Ok(d) => Err(Ok(diag_output(&d))),
            Err(_) => Err(Err(e)),
        },
        Err(e) => Err(Err(e)),
    }
}

macro_rules! load_or_return {
    ($p:expr) => {
        match or_diag(load($p)) {
            Ok(l) => l,
            Err(r) => return r,
        }
    };
}

pub fn validate(path: &Path) -> CmdResult {
    let l = load_or_return!(path);
    let mut d = l.diags.clone();
    // validate every export too (geometry-dependent checks)
    for (_, e) in pipeline::exports(&l.project) {
        let p = pipeline::for_export(&l.project, &e);
        let (_, d2) = cutline_core::resolve(&p, &l.media, &pipeline::context(&l.dir));
        for x in d2 {
            if !d.contains(&x) {
                d.push(x);
            }
        }
    }
    d.extend(cutline_render::extras::warnings(&l));
    Ok(diag_output(&d))
}

pub fn schema() -> CmdResult {
    let s = cutline_core::schema_json();
    let v: serde_json::Value = serde_json::from_str(&s).map_err(CliError::internal)?;
    Ok(Output::ok(v, s))
}

const INIT_TEMPLATE: &str = r#"// cutline project — see `cutline guide` and `cutline schema`
{
  version: 1,
  name: "{{name}}",
  target: "{{target}}",
  brand: "brand/default.json",
  assets: {
{{assets}}  },
  main: [
{{main}}  ],
  layers: [
    { type: "progress_bar", style: "brand" },
  ],
  captions: { source: "auto", style: "{{caption_style}}" },
  audio: { loudness: { integrated: -14, true_peak: -1.0 }, voice_chain: "clean_voice" },
}
"#;

pub fn init(dir: &Path, target: &str, assets_dir: Option<&Path>, name: Option<&str>) -> CmdResult {
    std::fs::create_dir_all(dir).map_err(CliError::usage)?;
    for sub in ["footage", "audio", "gen", "sfx", "brand", "out"] {
        std::fs::create_dir_all(dir.join(sub)).map_err(CliError::internal)?;
    }
    let brand_src = cutline_deps::paths::assets_dir().join("brand").join("default.json");
    let brand_dst = dir.join("brand").join("default.json");
    if !brand_dst.exists() {
        let b = std::fs::read_to_string(&brand_src).unwrap_or_else(|_| "{}".into());
        std::fs::write(&brand_dst, b).map_err(CliError::internal)?;
    }
    let mut assets = String::new();
    let mut main = String::new();
    let mut found = vec![];
    if let Some(ad) = assets_dir {
        let mut files: Vec<PathBuf> = std::fs::read_dir(ad)
            .map_err(CliError::usage)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for (i, f) in files.iter().enumerate() {
            let kind = cutline_core::timeline::default_kind_from_ext(&f.to_string_lossy());
            let rel = pathdiff(f, dir);
            let id = format!("a{}", i + 1);
            let role = match kind {
                cutline_core::model::AssetKind::Audio => {
                    let n = f.file_name().unwrap().to_string_lossy().to_lowercase();
                    if n.contains("music") {
                        ", role: \"music\""
                    } else if n.contains("sfx") || n.contains("whoosh") {
                        ", role: \"sfx\""
                    } else {
                        ", role: \"voice\""
                    }
                }
                _ => "",
            };
            assets.push_str(&format!("    {id}: {{ path: {:?}{role} }},\n", rel));
            if matches!(kind, cutline_core::model::AssetKind::Video) {
                main.push_str(&format!("    {{ id: \"clip{}\", clip: \"{id}\" }},\n", i + 1));
            }
            found.push(json!({"id": id, "path": rel, "kind": format!("{kind:?}").to_lowercase()}));
        }
    }
    let name = name.map(str::to_string).unwrap_or_else(|| {
        dir.canonicalize()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "video".into())
    });
    let style = if target == "shorts" { "bold_pop" } else { "clean_sentence" };
    let text = INIT_TEMPLATE
        .replace("{{name}}", &name)
        .replace("{{target}}", target)
        .replace("{{assets}}", &assets)
        .replace("{{main}}", &main)
        .replace("{{caption_style}}", style);
    let pf = dir.join("project.json5");
    if pf.exists() {
        return Err(CliError::usage(anyhow::anyhow!("{} already exists", pf.display())));
    }
    std::fs::write(&pf, text).map_err(CliError::internal)?;
    Ok(Output::ok(
        json!({"ok": true, "project": pf, "assets": found}),
        format!("created {}\nnext: edit `main`, then `cutline validate {}`", pf.display(), pf.display()),
    ))
}

fn pathdiff(p: &Path, base: &Path) -> String {
    let (Ok(p), Ok(b)) = (p.canonicalize(), base.canonicalize()) else {
        return p.to_string_lossy().replace('\\', "/");
    };
    let pc: Vec<_> = p.components().collect();
    let bc: Vec<_> = b.components().collect();
    let common = pc.iter().zip(bc.iter()).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..bc.len() {
        out.push("..");
    }
    for c in &pc[common..] {
        out.push(c);
    }
    out.to_string_lossy().replace('\\', "/")
}

pub fn expand(path: &Path) -> CmdResult {
    let l = load_or_return!(path);
    if has_errors(&l.diags) {
        return Ok(diag_output(&l.diags));
    }
    let brand = cutline_render::extras::load_brand(&l, &l.project).map_err(CliError::internal)?;
    let (expanded, notes) = cutline_render::auto::expand(&l, &l.project, &brand).map_err(CliError::internal)?;
    let (tl, d) = cutline_core::resolve(&expanded, &l.media, &pipeline::context(&l.dir));
    if has_errors(&d) {
        return Ok(diag_output(&d));
    }
    let tl = tl.ok_or_else(|| CliError::internal(anyhow::anyhow!("no timeline")))?;
    let edl = edl_table(&tl);
    let pj = serde_json::to_value(&expanded).map_err(CliError::internal)?;
    let text = serde_json::to_string_pretty(&pj).unwrap_or_default();
    let note_lines: String = notes
        .iter()
        .map(|n| format!("\n· {} {}: {}", n.code, if n.path.is_empty() { "/" } else { &n.path }, n.message))
        .collect();
    let note_block = if notes.is_empty() { String::new() } else { format!("\n\nAuto directives:{note_lines}") };
    Ok(Output::ok(
        json!({"project": pj, "edl": edl_json(&tl), "duration": tl.duration(), "frames": tl.total_frames, "notes": notes}),
        format!("{text}\n\n{edl}{note_block}"),
    ))
}

pub fn edl_json(tl: &cutline_core::Timeline) -> serde_json::Value {
    json!({
        "fps": tl.fps.ff(),
        "clips": tl.clips.iter().map(|c| json!({
            "id": c.id, "asset": c.asset, "tl_in": tl.secs(c.start), "tl_out": tl.secs(c.end()),
            "frame_in": c.start, "frame_out": c.end(), "src_in": c.src_in, "src_out": c.src_out, "speed": c.speed,
            "effects": c.clip.effects.len(), "generated_by": c.clip.generated_by,
        })).collect::<Vec<_>>(),
        "transitions": tl.transitions.iter().map(|t| json!({"type": t.transition.kind, "at": tl.secs(t.start), "duration": tl.secs(t.frames), "after": tl.clips[t.after].id})).collect::<Vec<_>>(),
        "layers": tl.layers.iter().map(|l| json!({"index": l.index, "type": l.layer.kind(), "start": tl.secs(l.start), "end": tl.secs(l.end), "z": l.z})).collect::<Vec<_>>(),
    })
}

pub fn edl_table(tl: &cutline_core::Timeline) -> String {
    let mut s = format!("EDL  ({} fps, {} frames, {})\n", tl.fps, tl.total_frames, format_tc(tl.duration()));
    s.push_str(&format!(
        "{:<4} {:<16} {:<10} {:<10} {:<10} {:<10} {:<10} {:<6} {}\n",
        "#", "id", "asset", "tl_in", "tl_out", "src_in", "src_out", "speed", "notes"
    ));
    for c in &tl.clips {
        let mut notes = vec![];
        if !c.clip.effects.is_empty() {
            notes.push(format!("{} fx", c.clip.effects.len()));
        }
        if let Some(t) = tl.transitions.iter().find(|t| t.after == c.index) {
            notes.push(format!("→ {} {:.2}s", t.transition.kind, tl.secs(t.frames)));
        }
        if let Some(g) = &c.clip.generated_by {
            notes.push(format!("[{g}]"));
        }
        s.push_str(&format!(
            "{:<4} {:<16} {:<10} {:<10} {:<10} {:<10} {:<10} {:<6} {}\n",
            c.index + 1,
            c.id,
            c.asset,
            format_tc(tl.secs(c.start)),
            format_tc(tl.secs(c.end())),
            format_tc(c.src_in),
            format_tc(c.src_out),
            format!("{:.2}", c.speed),
            notes.join(", ")
        ));
    }
    if !tl.layers.is_empty() {
        s.push_str("\nLayers\n");
        for l in &tl.layers {
            s.push_str(&format!(
                "  [{}] {:<12} {} – {}  z={}\n",
                l.index,
                l.layer.kind(),
                format_tc(tl.secs(l.start)),
                format_tc(tl.secs(l.end)),
                l.z
            ));
        }
    }
    s
}

pub struct RenderArgs {
    pub export: Option<String>,
    pub draft: bool,
    pub dry_run: bool,
    pub no_cache: bool,
    pub allow_qa_errors: bool,
    pub hw: bool,
    pub jobs: usize,
    pub out: Option<PathBuf>,
}

pub fn render(path: &Path, a: RenderArgs) -> CmdResult {
    let l = load_or_return!(path);
    if has_errors(&l.diags) {
        return Ok(diag_output(&l.diags));
    }
    let tc = cutline_deps::toolchain().tool_err()?;
    let opts = RenderOptions {
        draft: a.draft,
        no_cache: a.no_cache,
        jobs: a.jobs,
        hw: a.hw,
        out: a.out.clone(),
        skip_loudness: false,
    };
    if a.dry_run {
        let mut text = String::new();
        let mut plans = vec![];
        for (name, e) in pipeline::exports(&l.project) {
            if let Some(o) = &a.export {
                if &pipeline::export_name(&e) != o {
                    continue;
                }
            }
            let pr = pipeline::prepare(&l, name.clone(), &e, a.draft).map_err(CliError::validation)?;
            let plan = pipeline::plan_for(&l, &pr, &opts).map_err(CliError::internal)?;
            text.push_str(&cutline_render::dry_run_text(tc, &plan));
            plans.push(json!({
                "export": name, "output": plan.output, "segments": plan.segments.iter().map(|s| json!({"clip": s.clip_id, "key": s.key, "mezzanine": s.mezzanine, "cached": s.mezzanine.exists(), "command": s.job.display(&tc.ffmpeg.to_string_lossy()), "filter": s.spec.filter})).collect::<Vec<_>>(),
                "final": {"command": plan.final_job.display(&tc.ffmpeg.to_string_lossy()), "scripts": plan.final_job.scripts},
                "audio": cutline_render::audio_plan::jobs_for_display(&plan.audio).iter().map(|j| json!({"command": j.display(&tc.ffmpeg.to_string_lossy()), "scripts": j.scripts})).collect::<Vec<_>>(),
            }));
        }
        return Ok(Output::ok(json!({"dry_run": true, "plans": plans}), text));
    }
    let results = cutline_qa::deliver::render_all(&l, &opts, a.export.as_deref()).map_err(classify)?;
    let mut human = vec![];
    let mut qa_err = false;
    for r in &results {
        qa_err |= !r.qa.ok;
        human.push(format!(
            "rendered {} ({:.2}s) — segments: {} cached, {} rendered",
            r.output.display(),
            r.duration,
            r.exec.cache_hits,
            r.exec.cache_misses
        ));
        for d in &r.diagnostics {
            let hint = d.hint.as_deref().map(|h| format!(" (hint: {h})")).unwrap_or_default();
            human.push(format!("⚠ {} {}: {}{hint}", d.code, d.path, d.message));
        }
        human.push(cutline_qa::qa::summary(&r.qa));
        human.push(format!(
            "  qa: {}\n  report: {}\n  srt: {}\n  credits: {}",
            r.qa_path.display(),
            r.report_path.display(),
            r.srt_path.display(),
            r.credits_path.display()
        ));
    }
    human.push(cutline_qa::deliver::PUBLISHING_CHECKLIST.to_string());
    let code = if qa_err && !a.allow_qa_errors { EXIT_VALIDATION } else { EXIT_OK };
    Ok(Output::ok(
        json!({"ok": code == 0, "exports": results, "publishing_checklist": cutline_qa::deliver::PUBLISHING_CHECKLIST}),
        human.join("\n"),
    )
    .with_code(code))
}

fn classify(e: anyhow::Error) -> CliError {
    if e.downcast_ref::<LoadError>().is_some() {
        CliError::validation(e)
    } else if format!("{e:#}").contains("ffmpeg failed") {
        CliError::tool(e)
    } else {
        CliError::internal(e)
    }
}

pub fn preview(path: &Path, at: &[String], range: Option<&str>, safe_zones: bool, out: Option<PathBuf>) -> CmdResult {
    let l = load_or_return!(path);
    if has_errors(&l.diags) {
        return Ok(diag_output(&l.diags));
    }
    let tc = cutline_deps::toolchain().tool_err()?;
    let (name, e) = pipeline::exports(&l.project).into_iter().next().unwrap();
    // stills only: HTML that is on screen at none of the requested times need not be captured
    // frame by frame
    let stills: Option<Vec<f64>> = match (&l.timeline, range) {
        (Some(tl), None) if !at.is_empty() => at
            .iter()
            .map(|t| cutline_core::time::parse_time(t).map(|v| v.seconds(tl.fps, tl.duration())))
            .collect::<Result<Vec<f64>, String>>()
            .ok(),
        _ => None,
    };
    let pr = pipeline::prepare_for(&l, name, &e, true, stills.as_deref()).map_err(CliError::validation)?;
    let out_dir = out.unwrap_or_else(|| l.dir.join(".cutline").join("preview"));
    std::fs::create_dir_all(&out_dir).map_err(CliError::internal)?;
    let proxy = out_dir.join("preview_540p.mp4");
    let opts = RenderOptions { draft: true, out: Some(proxy.clone()), skip_loudness: true, ..Default::default() };
    let (plan, rep) = pipeline::render(&l, &pr, &opts).map_err(classify)?;
    let _ = std::fs::remove_dir_all(&plan.work_dir);
    let mut files = vec![];
    let fps = pr.timeline.fps;
    for t in at {
        let tv = cutline_core::time::parse_time(t).map_err(|e| CliError::usage(anyhow::anyhow!(e)))?;
        let secs = tv.seconds(fps, pr.timeline.duration()).min(pr.timeline.duration() - fps.frames_to_secs(1)).max(0.0);
        let png = out_dir.join(format!("at_{}.png", format_tc(secs).replace(':', "-")));
        let mut vf = String::new();
        if safe_zones {
            vf = safe_zone_filter(&pr.timeline.target, &plan.geometry);
        }
        let mut c = tc.ffmpeg_cmd();
        c.args(["-ss", &format!("{secs:.6}"), "-i"]).arg(&proxy);
        if !vf.is_empty() {
            c.args(["-vf", &vf]);
        }
        c.args(["-frames:v", "1"]).arg(&png);
        let o = c.output().map_err(CliError::tool)?;
        if !o.status.success() {
            return Err(CliError::tool(anyhow::anyhow!("frame grab failed: {}", String::from_utf8_lossy(&o.stderr))));
        }
        files.push(png);
    }
    let mut clip = None;
    if let Some(r) = range {
        let (a, b) = r
            .split_once('-')
            .or_else(|| r.split_once(','))
            .ok_or_else(|| CliError::usage(anyhow::anyhow!("range must be a-b")))?;
        let ta = cutline_core::time::parse_time(a)
            .map_err(|e| CliError::usage(anyhow::anyhow!(e)))?
            .seconds(fps, pr.timeline.duration());
        let tb = cutline_core::time::parse_time(b)
            .map_err(|e| CliError::usage(anyhow::anyhow!(e)))?
            .seconds(fps, pr.timeline.duration());
        let mp4 =
            out_dir.join(format!("range_{}_{}.mp4", format_tc(ta).replace(':', "-"), format_tc(tb).replace(':', "-")));
        let mut c = tc.ffmpeg_cmd();
        c.args(["-ss", &format!("{ta:.6}"), "-i"]).arg(&proxy).args(["-t", &format!("{:.6}", tb - ta)]);
        if safe_zones {
            c.args(["-vf", &safe_zone_filter(&pr.timeline.target, &plan.geometry)]);
        }
        c.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-c:a", "aac"]).arg(&mp4);
        let o = c.output().map_err(CliError::tool)?;
        if !o.status.success() {
            return Err(CliError::tool(anyhow::anyhow!("range cut failed: {}", String::from_utf8_lossy(&o.stderr))));
        }
        clip = Some(mp4);
    }
    Ok(Output::ok(
        json!({"ok": true, "proxy": proxy, "frames": files, "clip": clip, "cache_hits": rep.cache_hits}),
        format!(
            "proxy: {}\n{}{}",
            proxy.display(),
            files.iter().map(|f| format!("frame: {}\n", f.display())).collect::<String>(),
            clip.map(|c| format!("clip: {}", c.display())).unwrap_or_default()
        ),
    ))
}

/// drawbox overlay showing the target's safe zone (scaled to the preview geometry).
pub fn safe_zone_filter(t: &cutline_core::target::TargetSpec, g: &cutline_render::Geometry) -> String {
    let sx = g.width as f64 / t.width as f64;
    let (x, y, w, h) = t.safe_rect();
    format!(
        "drawbox=x={:.0}:y={:.0}:w={:.0}:h={:.0}:color=lime@0.9:t=3,drawbox=x=0:y=0:w=iw:h={:.0}:color=red@0.25:t=fill,drawbox=x=0:y={:.0}:w=iw:h=ih-{:.0}:color=red@0.25:t=fill,drawbox=x={:.0}:y=0:w=iw-{:.0}:h=ih:color=red@0.25:t=fill",
        x * sx,
        y * sx,
        w * sx,
        h * sx,
        y * sx,
        (y + h) * sx,
        (y + h) * sx,
        (x + w) * sx,
        (x + w) * sx
    )
}

pub fn qa(video: &Path, target: &str, project: Option<&Path>) -> CmdResult {
    let tc = cutline_deps::toolchain().tool_err()?;
    let tn: cutline_core::model::TargetName = serde_json::from_value(json!(target))
        .map_err(|_| CliError::usage(anyhow::anyhow!("unknown target `{target}`")))?;
    let (spec, ctx) = match project {
        Some(pp) => {
            let l = load_or_return!(pp);
            let (name, e) = pipeline::exports(&l.project)
                .into_iter()
                .find(|(_, e)| e.target == tn)
                .unwrap_or((None, cutline_core::model::Export { target: tn, ..Default::default() }));
            let pr = pipeline::prepare(&l, name, &e, false).map_err(CliError::validation)?;
            (pr.timeline.target.clone(), cutline_qa::deliver::qa_context(&pr.project, &pr.timeline, &pr.extras))
        }
        None => {
            let probe = cutline_media::probe(tc, video, None).map_err(CliError::usage)?;
            let fps = probe.video.as_ref().and_then(|v| cutline_core::time::Fps::parse(&v.fps));
            let spec = cutline_core::target::resolve(
                tn,
                None,
                fps,
                probe.video.as_ref().map(|v| v.width),
                probe.video.as_ref().map(|v| v.height),
            );
            let ctx = cutline_qa::qa::QaContext {
                is_shorts: spec.is_vertical(),
                max_cps: 20.0,
                loudness_target: Some((-14.0, -1.0)),
                ..Default::default()
            };
            (spec, ctx)
        }
    };
    let r = cutline_qa::qa::run_qa(tc, video, &spec, &ctx).map_err(CliError::tool)?;
    let qa_path = video.with_extension("qa.json");
    let _ = std::fs::write(&qa_path, serde_json::to_vec_pretty(&r).unwrap_or_default());
    let code = if r.ok { EXIT_OK } else { EXIT_VALIDATION };
    Ok(Output::ok(json!(r), cutline_qa::qa::summary(&r)).with_code(code))
}

pub fn review(path: &Path, out: &Path) -> CmdResult {
    let l = load_or_return!(path);
    if has_errors(&l.diags) {
        return Ok(diag_output(&l.diags));
    }
    let out = if out.is_absolute() { out.to_path_buf() } else { l.dir.join(out) };
    let r = cutline_qa::review::review(&l, &out).map_err(classify)?;
    let mut h = format!(
        "review pack: {}\nsource: {}\nQA: {}\n",
        r.dir.display(),
        r.source_video.display(),
        if r.qa_ok { "PASS" } else { "FAIL" }
    );
    h.push_str("images:\n");
    for i in &r.images {
        h.push_str(&format!("  {}\n", i.display()));
    }
    h.push_str("suggestions:\n");
    for s in &r.suggestions {
        h.push_str(&format!("  - {s}\n"));
    }
    Ok(Output::ok(json!(r), h))
}

pub fn cues(path: &Path) -> CmdResult {
    let l = load_or_return!(path);
    let mut h = vec![];
    if l.cues.is_empty() {
        h.push("no cue references (define `cues` and use \"cue:name\" in timeline times)".to_string());
    }
    for c in &l.cues {
        h.push(format!(
            "{:<16} {}  \"{}\"{}  ({} {:.2}–{:.2}s)",
            c.name,
            format_tc(c.at),
            c.matched,
            if c.exact { "" } else { " ~fuzzy" },
            c.asset,
            c.source_start,
            c.source_end
        ));
    }
    for x in l.diags.iter().filter(|x| x.code.contains("CUE")) {
        h.push(format!("⚠ {} {}: {}", x.code, x.path, x.message));
    }
    Ok(Output::ok(
        json!({"cues": l.cues, "warnings": l.diags.iter().filter(|x| x.code.contains("CUE")).collect::<Vec<_>>()}),
        h.join("\n"),
    ))
}

pub fn beats(path: &Path, asset: Option<&str>) -> CmdResult {
    use cutline_core::model::{Layer, Role};
    let l = load_or_return!(path);
    let id = match asset {
        Some(a) => a.to_string(),
        None => l
            .project
            .layers
            .iter()
            .find_map(|x| match x {
                Layer::Audio { asset, role, .. }
                    if role.or(l.project.assets.get(asset).and_then(|a| a.role)).unwrap_or(Role::Music)
                        == Role::Music =>
                {
                    Some(asset.clone())
                }
                _ => None,
            })
            .ok_or_else(|| CliError::usage(anyhow::anyhow!("no music audio layer; pass --asset")))?,
    };
    let b = cutline_render::auto::beats_for(&l, &id).map_err(CliError::tool)?;
    // the same beats on the timeline (what `bar:N` / `beat:N` and snap_to_beats use)
    let raw = std::fs::read_to_string(&l.path).ok().and_then(|t| cutline_core::parse::parse_json5(&t).ok());
    let grid = raw.as_ref().and_then(|v| cutline_render::beatrefs::grid(v, &l.dir).ok()).filter(|g| g.asset == id);
    let list = |v: &[f64], n: usize| v.iter().take(n).map(|t| format!("{t:.3}")).collect::<Vec<_>>().join(" ");
    let h = match &b {
        Some(b) => {
            let clear = b.bar_confidence >= cutline_render::beatrefs::BAR_CONFIDENT;
            let mut h = vec![format!(
                "{id}: {:.2} BPM{}, {} beats, {} bars (bar starts {}, confidence {:.2})",
                b.bpm,
                if b.steady { " (steady: exact grid)" } else { " (tempo moves: tracked beats)" },
                b.times.len(),
                b.downbeats.len(),
                if clear { "clear" } else { "uncertain" },
                b.bar_confidence
            )];
            match &grid {
                Some(g) => {
                    h.push(format!("timeline bars 1–{} (s): {}", g.bars.len().min(16), list(&g.bars, 16)));
                    h.push(format!("timeline beats 1–{} (s): {}", g.beats.len().min(16), list(&g.beats, 16)));
                    h.push("use `end_at: \"bar:5\"`, `\"bar:5.3\"` (third beat of bar 5) or `\"beat:17\"`; `auto: {snap_to_beats: {}}` moves cuts onto them".into());
                }
                None => {
                    h.push(format!("source bars (s): {}", list(&b.downbeats, 16)));
                    h.push(format!("source beats (s): {}", list(&b.times, 16)));
                }
            }
            h.join("\n")
        }
        None => format!("{id}: no steady beat found"),
    };
    Ok(Output::ok(json!({"asset": id, "beats": b, "timeline": grid}), h))
}

pub fn record(path: &Path, asset: Option<&str>) -> CmdResult {
    let done = cutline_render::terminal::rerecord(path, asset).map_err(CliError::tool)?;
    let h: Vec<String> = done.iter().map(|(id, p)| format!("✔ {id}: recorded {}", p.display())).collect();
    let j: Vec<_> = done.iter().map(|(id, p)| json!({"asset": id, "recording": p})).collect();
    Ok(Output::ok(json!({"recorded": j}), h.join("\n")))
}

pub fn chapters(path: &Path) -> CmdResult {
    let l = load_or_return!(path);
    if has_errors(&l.diags) {
        return Ok(diag_output(&l.diags));
    }
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).map_err(classify)?;
    let (ch, warn) = cutline_qa::publish::chapters(&pr);
    let text = cutline_qa::publish::chapters_text(&ch);
    let mut h = text.clone();
    for w in &warn {
        h.push_str(&format!("\n⚠ {}", w.message));
    }
    Ok(Output::ok(json!({"chapters": ch, "text": text, "warnings": warn}), h))
}

pub fn thumbnail(path: &Path, at: &str, text: Option<&str>, template: &str, out: Option<PathBuf>) -> CmdResult {
    let l = load_or_return!(path);
    if has_errors(&l.diags) {
        return Ok(diag_output(&l.diags));
    }
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).map_err(classify)?;
    let t = cutline_core::time::parse_time(at)
        .map_err(|e| CliError::usage(anyhow::anyhow!(e)))?
        .seconds(pr.timeline.fps, pr.timeline.duration());
    let out = out.unwrap_or_else(|| {
        l.dir.join("out").join(format!("{}-thumbnail.jpg", cutline_render::output_name(&l.project, &l.path)))
    });
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(CliError::internal)?;
    }
    let p = cutline_qa::publish::thumbnail(&l, &pr, t, text, template, &out).map_err(classify)?;
    Ok(Output::ok(json!({"ok": true, "thumbnail": p}), format!("thumbnail: {}", p.display())))
}
