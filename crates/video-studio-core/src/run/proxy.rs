//! The `cutline` proxy. During a run, the `cutline` on Claude's PATH is the app's helper under
//! another name. It runs the real cutline with the same arguments and working directory, passes
//! its output through, and keeps the per-invocation logs the spec asks for (§15):
//! `logs/cutline/<n>-<command>.log` with argv, exit code and duration. It also appends short
//! activity lines the orchestrator turns into progress events.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use camino::Utf8PathBuf;
use serde_json::json;

use crate::claude::broker::append_line;
use crate::claude::Mode;
use crate::eleven::HelperContext;
use crate::timeutil;

/// The cutline subcommand in an argument list: the first argument that isn't an option.
pub fn subcommand(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--variant" || a == "--locale" {
            i += 2;
        } else if a.starts_with('-') {
            i += 1;
        } else {
            return Some(a);
        }
    }
    None
}

pub fn activity_path(ctx: &HelperContext) -> Utf8PathBuf {
    ctx.logs_dir.join("cutline/activity.ndjson")
}

/// Creates the next `logs/cutline/<n>-<command>.log`, numbering from 1.
fn open_log(ctx: &HelperContext, command: &str) -> Option<(u32, Utf8PathBuf, std::fs::File)> {
    let dir = ctx.logs_dir.join("cutline");
    std::fs::create_dir_all(&dir).ok()?;
    let mut n = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".log")).count() as u32)
        .unwrap_or(0)
        + 1;
    loop {
        let path = dir.join(format!("{n:03}-{command}.log"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Some((n, path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(_) => return None,
        }
        if n > 100_000 {
            return None;
        }
    }
}

/// Runs the real cutline and returns its exit code.
pub fn run(ctx: &HelperContext, args: &[String]) -> i32 {
    let Some(real) = ctx.cutline.clone() else {
        eprintln!("cutline isn't configured for this run");
        return 127;
    };
    let command = subcommand(args).unwrap_or("help").to_string();
    // The broker already refuses these; the proxy refuses them too so a dry run can't render
    // even if a command slipped past.
    if ctx.mode == Mode::DryRun && matches!(command.as_str(), "render" | "thumbnail" | "record") {
        eprintln!("`cutline {command}` isn't available in a dry run: a dry run validates and reviews, it never renders finals.");
        return 2;
    }
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let log = open_log(ctx, &command.replace(|c: char| !c.is_ascii_alphanumeric(), "-"));
    let (n, log_file) = match log {
        Some((n, _, f)) => (n, Some(f)),
        None => (0, None),
    };
    let log_file = Arc::new(Mutex::new(log_file));
    let write_log = |text: &str| {
        if let Some(f) = log_file.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{text}");
        }
    };
    let started = Instant::now();
    write_log(&format!(
        "# argv: {}",
        serde_json::to_string(&std::iter::once(real.to_string()).chain(args.iter().cloned()).collect::<Vec<_>>())
            .unwrap_or_default()
    ));
    write_log(&format!("# cwd: {cwd}"));
    write_log(&format!("# started: {}", timeutil::now_rfc3339()));
    let activity = activity_path(ctx);
    append_line(
        &activity,
        &json!({"ts": timeutil::now_rfc3339(), "type": "start", "n": n, "command": command, "argv": args}).to_string(),
    );

    let mut child = match Command::new(real.as_str())
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("couldn't start cutline ({real}): {e}");
            write_log(&format!("# failed to start: {e}"));
            append_line(&activity, &json!({"ts": timeutil::now_rfc3339(), "type": "exit", "n": n, "command": command, "code": 127, "seconds": 0.0}).to_string());
            return 127;
        }
    };
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let log_out = log_file.clone();
    let t_out = std::thread::spawn(move || {
        let stdout = std::io::stdout();
        for line in BufReader::new(out).split(b'\n').map_while(|l| l.ok()) {
            let mut o = stdout.lock();
            let _ = o.write_all(&line);
            let _ = o.write_all(b"\n");
            let _ = o.flush();
            if let Some(f) = log_out.lock().unwrap().as_mut() {
                let _ = f.write_all(b"[out] ");
                let _ = f.write_all(&line);
                let _ = f.write_all(b"\n");
            }
        }
    });
    let log_err = log_file.clone();
    let activity_err = activity.clone();
    let t_err = std::thread::spawn(move || {
        let stderr = std::io::stderr();
        let mut last = Instant::now() - std::time::Duration::from_secs(1);
        // ffmpeg-style progress uses carriage returns: treat them as line ends too.
        let mut reader = BufReader::new(err);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let mut byte = [0u8; 1];
            let mut eof = false;
            loop {
                match std::io::Read::read(&mut reader, &mut byte) {
                    Ok(0) | Err(_) => {
                        eof = true;
                        break;
                    }
                    Ok(_) if byte[0] == b'\n' || byte[0] == b'\r' => break,
                    Ok(_) => buf.push(byte[0]),
                }
            }
            if !buf.is_empty() {
                let mut e = stderr.lock();
                let _ = e.write_all(&buf);
                let _ = e.write_all(b"\n");
                if let Some(f) = log_err.lock().unwrap().as_mut() {
                    let _ = f.write_all(b"[err] ");
                    let _ = f.write_all(&buf);
                    let _ = f.write_all(b"\n");
                }
                // A few lines a second is plenty for a progress display.
                if last.elapsed().as_millis() >= 250 && buf.len() <= 400 {
                    last = Instant::now();
                    let text = String::from_utf8_lossy(&buf).trim().to_string();
                    if !text.is_empty() {
                        append_line(&activity_err, &json!({"type": "line", "n": n, "text": text}).to_string());
                    }
                }
            }
            if eof {
                break;
            }
        }
        let _ = reader.fill_buf();
    });
    let status = child.wait();
    let _ = t_out.join();
    let _ = t_err.join();
    let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
    let seconds = started.elapsed().as_secs_f64();
    write_log(&format!("# exit: {code}"));
    write_log(&format!("# duration: {seconds:.2} s"));
    append_line(&activity, &json!({"ts": timeutil::now_rfc3339(), "type": "exit", "n": n, "command": command, "code": code, "seconds": seconds}).to_string());
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_subcommand() {
        let a = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(subcommand(&a("--json validate p.json5")), Some("validate"));
        assert_eq!(subcommand(&a("--variant teaser -v render p.json5")), Some("render"));
        assert_eq!(subcommand(&a("--version")), None);
    }

    #[test]
    fn logs_each_invocation_and_blocks_render_in_dry_runs() {
        let d = tempfile::tempdir().unwrap();
        let dir = camino::Utf8Path::from_path(d.path()).unwrap();
        #[cfg(unix)]
        let fake = {
            use std::os::unix::fs::PermissionsExt;
            let fake = dir.join("real-cutline");
            std::fs::write(&fake, "#!/bin/sh\necho \"out $*\"\necho \"segment 1/2\" >&2\nexit 3\n").unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            fake
        };
        #[cfg(windows)]
        let fake = {
            let fake = dir.join("real-cutline.cmd");
            std::fs::write(&fake, "@echo out %*\r\n@echo segment 1/2 1>&2\r\n@exit /b 3\r\n").unwrap();
            fake
        };
        let run = dir.join("run");
        std::fs::create_dir_all(run.join("logs")).unwrap();
        let mut ctx = HelperContext {
            run_id: "r".into(),
            run_folder: run.clone(),
            mode: Mode::Production,
            logs_dir: run.join("logs"),
            cutline: Some(fake),
            credit_cap: 0,
            cache_dir: dir.join("cache"),
            cache_limit_bytes: 0,
            default_voice: None,
            default_voice_settings: None,
            default_model: None,
            voice_preference: Default::default(),
            voice_gender: Default::default(),
            voice_provider: Default::default(),
            cartesia_base_url: None,
            cartesia_test_key: None,
            base_url: None,
            test_api_key: None,
            capture: None,
            relay_dir: None,
        };
        let args: Vec<String> = ["--json", "validate", "project.json5"].iter().map(|s| s.to_string()).collect();
        assert_eq!(run_proxy(&ctx, &args), 3);
        assert_eq!(run_proxy(&ctx, &args), 3);
        let log = std::fs::read_to_string(run.join("logs/cutline/001-validate.log")).unwrap();
        assert!(
            log.contains("# argv: [")
                && log.contains("\"validate\"")
                && log.contains("[out] out --json validate project.json5")
                && log.contains("[err] segment 1/2")
                && log.contains("# exit: 3")
                && log.contains("# duration:"),
            "{log}"
        );
        assert!(run.join("logs/cutline/002-validate.log").exists());
        let activity: Vec<serde_json::Value> = std::fs::read_to_string(activity_path(&ctx))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(activity.iter().filter(|a| a["type"] == "exit" && a["code"] == 3).count(), 2);
        assert!(activity.iter().any(|a| a["type"] == "line" && a["text"] == "segment 1/2"));
        ctx.mode = Mode::DryRun;
        let render: Vec<String> = ["render", "project.json5"].iter().map(|s| s.to_string()).collect();
        assert_eq!(run_proxy(&ctx, &render), 2);
        assert!(!run.join("logs/cutline/003-render.log").exists(), "nothing ran");
    }

    fn run_proxy(ctx: &HelperContext, args: &[String]) -> i32 {
        super::run(ctx, args)
    }
}
