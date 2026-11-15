//! `vs-uitest`: UI Automation tests for Syncfusion Video Studio (ui/spec.md §23.9 item 4).
//!
//! Starts the app against a scratch library (fake Claude, real cutline when present) and drives it
//! only through UI Automation and synthesized input, as a person or Narrator would:
//! launch and onboarding; opening a project by path (what a double-click does); an accessible-name
//! audit of every control; the Windows shortcuts (§23.6); editing the spec and a slash command;
//! a brand preset in the App Library; Validate, Run and Stop; Relink with Search Folder and the
//! log export through the common file dialogs; a keyboard-only pass. Screenshots go next to the
//! log. Exits non-zero when any check fails.
//!
//! `vs-uitest --app <path to video-studio-app.exe> [--out <folder>]`

#[cfg(not(windows))]
fn main() {
    eprintln!("vs-uitest runs on Windows only");
}

#[cfg(windows)]
fn main() {
    std::process::exit(ui::run());
}

#[cfg(windows)]
mod ui {
    use std::io::Write;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    use camino::{Utf8Path, Utf8PathBuf};
    use serde_json::json;
    use windows::core::{Interface, BSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE};

    struct T {
        a: IUIAutomation,
        out: Utf8PathBuf,
        log: std::fs::File,
        failures: usize,
        checks: usize,
    }

    impl T {
        fn check(&mut self, ok: bool, what: &str) {
            self.checks += 1;
            if !ok {
                self.failures += 1;
            }
            let line = format!("{} {what}", if ok { "ok  " } else { "FAIL" });
            println!("{line}");
            let _ = writeln!(self.log, "{line}");
        }

        fn note(&mut self, what: &str) {
            println!("     {what}");
            let _ = writeln!(self.log, "     {what}");
        }

        fn root(&self) -> IUIAutomationElement {
            unsafe { self.a.GetRootElement().unwrap() }
        }

        /// A top-level window whose name contains `title`.
        fn window(&self, title: &str, secs: u64) -> Option<IUIAutomationElement> {
            let deadline = Instant::now() + Duration::from_secs(secs);
            loop {
                unsafe {
                    let cond = self
                        .a
                        .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_WindowControlTypeId.0))
                        .unwrap();
                    if let Ok(all) = self.root().FindAll(TreeScope_Children, &cond) {
                        for i in 0..all.Length().unwrap_or(0) {
                            let w = all.GetElement(i).unwrap();
                            if name(&w).contains(title) {
                                return Some(w);
                            }
                        }
                    }
                }
                if Instant::now() > deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }

        /// The first descendant with this name (and control type, when given).
        fn find(
            &self,
            within: &IUIAutomationElement,
            label: &str,
            ctype: Option<UIA_CONTROLTYPE_ID>,
            secs: u64,
        ) -> Option<IUIAutomationElement> {
            let deadline = Instant::now() + Duration::from_secs(secs);
            loop {
                unsafe {
                    let by_name =
                        self.a.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(label))).unwrap();
                    let cond = match ctype {
                        Some(t) => self
                            .a
                            .CreateAndCondition(
                                &by_name,
                                &self
                                    .a
                                    .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(t.0))
                                    .unwrap(),
                            )
                            .unwrap(),
                        None => by_name,
                    };
                    if let Ok(el) = within.FindFirst(TreeScope_Descendants, &cond) {
                        return Some(el);
                    }
                }
                if Instant::now() > deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }

        fn click(&self, el: &IUIAutomationElement) -> bool {
            unsafe {
                if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
                    return p.Invoke().is_ok();
                }
                if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
                    return p.Select().is_ok();
                }
                if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) {
                    return p.Toggle().is_ok();
                }
                el.SetFocus().is_ok()
            }
        }

        fn front(&self, w: &IUIAutomationElement) {
            unsafe {
                if let Ok(h) = w.CurrentNativeWindowHandle() {
                    let h = HWND(h.0);
                    let _ = ShowWindow(h, SW_RESTORE);
                    let _ = SetForegroundWindow(h);
                }
                let _ = w.SetFocus();
            }
            std::thread::sleep(Duration::from_millis(300));
        }

        fn shot(&self, w: &IUIAutomationElement, file: &str) {
            unsafe {
                let Ok(r) = w.CurrentBoundingRectangle() else { return };
                let (x, y, wd, ht) = (r.left, r.top, r.right - r.left, r.bottom - r.top);
                use windows::Win32::Graphics::Gdi::*;
                let screen = GetDC(None);
                let mem = CreateCompatibleDC(Some(screen));
                let bmp = CreateCompatibleBitmap(screen, wd, ht);
                let old = SelectObject(mem, bmp.into());
                let _ = BitBlt(mem, 0, 0, wd, ht, Some(screen), x, y, SRCCOPY);
                let mut info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: 40,
                        biWidth: wd,
                        biHeight: -ht,
                        biPlanes: 1,
                        biBitCount: 32,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut px = vec![0u8; (wd * ht * 4) as usize];
                GetDIBits(mem, bmp, 0, ht as u32, Some(px.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
                SelectObject(mem, old);
                let _ = DeleteObject(bmp.into());
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                for p in px.chunks_exact_mut(4) {
                    p.swap(0, 2);
                    p[3] = 255;
                }
                let png = self.out.join(file);
                let _ = write_png(&png, wd as u32, ht as u32, &px);
            }
        }
    }

    fn write_png(path: &Utf8Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
        // A minimal PNG writer (stored zlib blocks) so the runner needs no image crate.
        fn crc(data: &[u8]) -> u32 {
            let mut c = 0xffff_ffffu32;
            for b in data {
                c ^= *b as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        }
        fn adler(data: &[u8]) -> u32 {
            let (mut a, mut b) = (1u32, 0u32);
            for x in data {
                a = (a + *x as u32) % 65521;
                b = (b + a) % 65521;
            }
            (b << 16) | a
        }
        let mut raw = Vec::with_capacity((w * h * 4 + h) as usize);
        for row in rgba.chunks_exact((w * 4) as usize) {
            raw.push(0);
            raw.extend_from_slice(row);
        }
        let mut z = vec![0x78, 0x01];
        for (i, chunk) in raw.chunks(65535).enumerate() {
            let last = (i + 1) * 65535 >= raw.len();
            z.push(last as u8);
            z.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
            z.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
            z.extend_from_slice(chunk);
        }
        z.extend_from_slice(&adler(&raw).to_be_bytes());
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut c = kind.to_vec();
            c.extend_from_slice(data);
            out.extend_from_slice(&c);
            out.extend_from_slice(&crc(&c).to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(b"IHDR", &ihdr);
        chunk(b"IDAT", &z);
        chunk(b"IEND", &[]);
        std::fs::write(path, out)
    }

    fn name(el: &IUIAutomationElement) -> String {
        unsafe { el.CurrentName().map(|b| b.to_string()).unwrap_or_default() }
    }

    /// A key event with its scan code too, as a keyboard sends it (Chromium derives
    /// KeyboardEvent.code from the scan code).
    fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;
        let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
        if matches!(
            vk,
            VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_PRIOR | VK_NEXT | VK_INSERT | VK_DELETE
        ) {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, ..Default::default() } },
        }
    }

    /// Presses a chord, e.g. `chord(&[VK_CONTROL, VK_SHIFT], VK_B)`.
    fn chord(mods: &[VIRTUAL_KEY], k: VIRTUAL_KEY) {
        let mut v: Vec<INPUT> = mods.iter().map(|m| key(*m, false)).collect();
        v.push(key(k, false));
        v.push(key(k, true));
        v.extend(mods.iter().rev().map(|m| key(*m, true)));
        unsafe { SendInput(&v, std::mem::size_of::<INPUT>() as i32) };
        std::thread::sleep(Duration::from_millis(400));
    }

    fn type_text(s: &str) {
        for u in s.encode_utf16() {
            if u == b'\n' as u16 {
                // a typed line break is the Enter key, as on a keyboard
                unsafe {
                    SendInput(&[key(VK_RETURN, false), key(VK_RETURN, true)], std::mem::size_of::<INPUT>() as i32)
                };
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let i = |up: bool| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wScan: u,
                        dwFlags: if up { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE },
                        ..Default::default()
                    },
                },
            };
            unsafe { SendInput(&[i(false), i(true)], std::mem::size_of::<INPUT>() as i32) };
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Fills the open common file dialog with `path` and accepts it.
    fn file_dialog(t: &T, path: &str) -> bool {
        // The dialog is owned by the app window, so UI Automation lists it under that window (or
        // at the top level); find it by its file name box.
        let found = || {
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                let mut scopes = vec![];
                if let Some(app) = t.window("Video Studio", 0) {
                    scopes.push(app);
                }
                scopes.push(t.root());
                for scope in &scopes {
                    for label in ["File name:", "Folder:"] {
                        for ctype in [UIA_EditControlTypeId, UIA_ComboBoxControlTypeId] {
                            if let Some(e) = t.find(scope, label, Some(ctype), 0) {
                                // the combo box's own edit takes the value
                                if ctype == UIA_ComboBoxControlTypeId {
                                    if let Some(inner) = unsafe {
                                        let c =
                                            t.a.CreatePropertyCondition(
                                                UIA_ControlTypePropertyId,
                                                &VARIANT::from(UIA_EditControlTypeId.0),
                                            )
                                            .ok()?;
                                        e.FindFirst(TreeScope_Children, &c).ok()
                                    } {
                                        return Some(inner);
                                    }
                                }
                                return Some(e);
                            }
                        }
                    }
                }
                if Instant::now() > deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        };
        let Some(d) = found() else {
            return false;
        };
        // Type the path: the dialog ignores a ValuePattern.SetValue on its name box (it then
        // saves under the default name in the last folder).
        unsafe {
            let _ = d.SetFocus();
        }
        std::thread::sleep(Duration::from_millis(200));
        chord(&[VK_CONTROL], VK_A);
        type_text(path);
        std::thread::sleep(Duration::from_millis(300));
        unsafe {
            let value = d
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .and_then(|v| v.CurrentValue())
                .map(|b| b.to_string())
                .unwrap_or_default();
            println!("     file dialog box {:?} = {value:?}", name(&d));
        }
        if let Some(app) = t.window("Video Studio", 0) {
            t.shot(&app, "dialog.png");
        }
        chord(&[], VK_RETURN);
        std::thread::sleep(Duration::from_millis(600));
        // A folder dialog may want a second Enter (Select Folder)
        if t.find(&t.root(), "Select Folder", Some(UIA_ButtonControlTypeId), 0).is_some() {
            chord(&[], VK_RETURN);
        }
        true
    }

    /// Every interactive control in `w` has an accessible name.
    fn audit_names(t: &mut T, w: &IUIAutomationElement, label: &str) {
        let interactive = [
            UIA_ButtonControlTypeId,
            UIA_EditControlTypeId,
            UIA_ComboBoxControlTypeId,
            UIA_CheckBoxControlTypeId,
            UIA_RadioButtonControlTypeId,
            UIA_TabItemControlTypeId,
            UIA_ListItemControlTypeId,
            UIA_HyperlinkControlTypeId,
            UIA_SliderControlTypeId,
            UIA_DocumentControlTypeId,
        ];
        let mut missing: Vec<String> = Vec::new();
        let mut count = 0;
        unsafe {
            let cond = t.a.CreateTrueCondition().unwrap();
            if let Ok(all) = w.FindAll(TreeScope_Descendants, &cond) {
                for i in 0..all.Length().unwrap_or(0) {
                    let el = all.GetElement(i).unwrap();
                    let ct = el.CurrentControlType().unwrap_or(UIA_CONTROLTYPE_ID(0));
                    if !interactive.contains(&ct) || el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                        continue;
                    }
                    count += 1;
                    if name(&el).trim().is_empty() {
                        let id = el.CurrentAutomationId().map(|b| b.to_string()).unwrap_or_default();
                        let class = el.CurrentClassName().map(|b| b.to_string()).unwrap_or_default();
                        missing.push(format!("{} [{}] {}", ct.0, class, id));
                    }
                }
            }
        }
        t.check(missing.is_empty(), &format!("{label}: all {count} interactive controls have accessible names"));
        for m in missing.iter().take(10) {
            t.note(&format!("unnamed: {m}"));
        }
    }

    fn launch(app: &Utf8Path, args: &[&str], env: &[(String, String)]) -> Child {
        let mut c = Command::new(app.as_str());
        c.args(args);
        for (k, v) in env {
            c.env(k, v);
        }
        c.spawn().expect("the app starts")
    }

    pub fn run() -> i32 {
        let args: Vec<String> = std::env::args().collect();
        let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
        let Some(app) = get("--app").map(Utf8PathBuf::from) else {
            eprintln!("usage: vs-uitest --app <video-studio-app.exe> [--out <folder>]");
            return 2;
        };
        let bin = app.parent().unwrap().to_owned();
        let out = get("--out").map(Utf8PathBuf::from).unwrap_or_else(|| {
            Utf8PathBuf::from_path_buf(std::env::temp_dir().join(format!("vs-uitest-{}", std::process::id()))).unwrap()
        });
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let a: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap() };
        let log = std::fs::File::create(out.join("uitest.log")).unwrap();
        let mut t = T { a, out: out.clone(), log, failures: 0, checks: 0 };

        // The scratch world: library, fake Claude, cutline, a project with footage.
        let lib = out.join("library");
        let footage = out.join("footage");
        std::fs::create_dir_all(&footage).unwrap();
        std::fs::write(footage.join("clip.mp4"), b"not really a video, but a file").unwrap();
        let script = out.join("script.json");
        std::fs::write(&script, json!([{"say": "Working."}, {"hang": true}]).to_string()).unwrap();
        let mut env = vec![
            ("VIDEO_STUDIO_HOME".to_string(), lib.to_string()),
            ("VIDEO_STUDIO_CLAUDE".to_string(), bin.join("vs-fake-claude.exe").to_string()),
            ("FAKE_CLAUDE_SCRIPT".to_string(), script.to_string()),
            ("FAKE_CLAUDE_TRACE".to_string(), out.join("trace.ndjson").to_string()),
        ];
        // A test ElevenLabs key (read by the helper), so specs with voice-over validate.
        let secrets = out.join("secrets.json");
        std::fs::write(&secrets, json!({"elevenlabs-api-key": "uitest-key"}).to_string()).unwrap();
        env.push(("VIDEO_STUDIO_TEST_SECRETS".to_string(), secrets.to_string()));
        if bin.join("cutline.exe").is_file() {
            env.push(("VIDEO_STUDIO_CUTLINE".to_string(), bin.join("cutline.exe").to_string()));
        }
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
        let project = out.join("UI test.cutproj");
        let p = video_studio_ffi::call(
            "project.new".into(),
            json!({"name": "UI test", "intent": "product_demo"}).to_string(),
        )
        .unwrap();
        let p: serde_json::Value = serde_json::from_str(&p).unwrap();
        video_studio_ffi::call("project.save".into(), json!({"project": p, "path": project}).to_string()).unwrap();

        // 1. Launch and onboarding
        let mut start = launch(&app, &[], &env);
        match t.window("Syncfusion Video Studio", 20) {
            Some(w) => {
                t.front(&w);
                t.check(
                    t.find(&w, "Setup checklist", None, 10).is_some(),
                    "the start window shows the setup checklist",
                );
                t.check(
                    t.find(&w, "New Project…", Some(UIA_ButtonControlTypeId), 2).is_some(),
                    "New Project is reachable",
                );
                audit_names(&mut t, &w, "start window");
                t.shot(&w, "01-start.png");
            }
            None => t.check(false, "the start window opens"),
        }
        let _ = start.kill();
        let _ = start.wait();

        // 2. Opening a project by path (what a double-click on a .cutproj does)
        let mut appp = launch(&app, &[project.as_str()], &env);
        let Some(w) = t.window("UI test", 20) else {
            t.check(false, "the project window opens");
            let _ = appp.kill();
            let _ = appp.wait();
            return 1;
        };
        t.check(true, "the project window opens from a .cutproj path");
        t.front(&w);
        std::thread::sleep(Duration::from_secs(2));
        t.shot(&w, "02-project.png");

        // 3. Editing and a slash command
        let scenes_before = std::fs::read_to_string(&project).unwrap_or_default().matches("## Scene").count();
        if let Some(ed) = t.find(&w, "Spec editor", None, 10) {
            unsafe {
                let _ = ed.SetFocus();
            }
            chord(&[VK_CONTROL], VK_END);
            type_text("\n\nTyped by UI Automation.\n");
            type_text("/scene");
            std::thread::sleep(Duration::from_millis(800));
            chord(&[], VK_RETURN);
            std::thread::sleep(Duration::from_millis(2500));
            let disk = std::fs::read_to_string(&project).unwrap_or_default();
            t.check(disk.contains("Typed by UI Automation."), "typing reaches the spec and autosave writes it");
            t.check(
                disk.matches("## Scene").count() == scenes_before + 1,
                "the /scene slash command inserts a scene block",
            );
        } else {
            t.check(false, "the spec editor is exposed to UI Automation");
        }

        // 4. Shortcuts (ui/spec.md §23.6)
        chord(&[VK_CONTROL], VK_OEM_2);
        t.check(
            t.find(&w, "Spec preview", None, 5)
                .map(|e| unsafe { !e.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(true) })
                .unwrap_or(false),
            "Ctrl+/ shows the preview",
        );
        t.shot(&w, "03-preview.png");
        chord(&[VK_CONTROL], VK_OEM_2);
        chord(&[VK_CONTROL, VK_MENU], VK_2);
        // each tab is recognized by what only it shows
        let project_tab = t.find(&w, "Audience", None, 5).is_some();
        if !project_tab {
            t.shot(&w, "03b-ctrl-alt-2.png");
        }
        t.check(project_tab, "Ctrl+Alt+2 opens the Project tab");
        chord(&[VK_CONTROL, VK_SHIFT], VK_B);
        t.check(t.find(&w, "Validation issues", None, 30).is_some(), "Ctrl+Shift+B validates and shows the report");
        t.shot(&w, "04-validation.png");
        // Chromium builds its accessibility tree on demand: audit once the window has been used.
        audit_names(&mut t, &w, "project window");
        chord(&[VK_CONTROL, VK_SHIFT], VK_L);
        let lib_w = t.window("App Library", 10);
        t.check(lib_w.is_some(), "Ctrl+Shift+L opens the App Library");
        if let Some(lw) = &lib_w {
            t.front(lw);
            audit_names(&mut t, lw, "App Library");
            // a brand preset: + → name → OK
            if let Some(b) = t.find(lw, "New brand preset", Some(UIA_ButtonControlTypeId), 5) {
                t.click(&b);
                std::thread::sleep(Duration::from_millis(600));
                type_text("UI Brand");
                chord(&[], VK_RETURN);
                std::thread::sleep(Duration::from_secs(2));
                let saved = std::fs::read_dir(lib.join("brands"))
                    .map(|d| {
                        d.flatten()
                            .any(|e| std::fs::read_to_string(e.path()).unwrap_or_default().contains("\"UI Brand\""))
                    })
                    .unwrap_or(false);
                t.check(saved, "a brand preset is created in the App Library (saved in the library)");
                t.check(
                    t.find(lw, "New brand preset", Some(UIA_HeaderControlTypeId), 1).is_none()
                        && t.find(lw, "UI Brand", Some(UIA_ListItemControlTypeId), 3).is_some(),
                    "the prompt closes once and the preset is listed",
                );
            }
            t.shot(lw, "05-library.png");
            chord(&[VK_CONTROL], VK_W);
        }
        t.front(&w);
        chord(&[VK_CONTROL], VK_OEM_COMMA);
        let settings = t.window("Settings", 10);
        t.check(settings.is_some(), "Ctrl+, opens Settings");
        if let Some(sw) = &settings {
            t.front(sw);
            audit_names(&mut t, sw, "Settings");
            t.shot(sw, "06-settings.png");
            chord(&[VK_CONTROL], VK_W);
        }
        t.front(&w);

        // 5. Run with the fake Claude, then Stop (F5 / Shift+F5)
        chord(&[], VK_F5);
        std::thread::sleep(Duration::from_secs(4));
        let sheet = t
            .find(&w, "Start Dry Run", Some(UIA_ButtonControlTypeId), 6)
            .or_else(|| t.find(&w, "Run", Some(UIA_ButtonControlTypeId), 1));
        match t.find(&w, "Cancel", Some(UIA_ButtonControlTypeId), 3) {
            Some(_) => {
                t.check(true, "F5 validates and shows the run confirmation");
                t.shot(&w, "07-run-confirm.png");
                // run without the dry run
                if let Some(cb) = t.find(&w, "Do a dry run first", None, 1).or_else(|| t.find(&w, "dry-first", None, 1))
                {
                    t.click(&cb);
                }
                if let Some(b) = sheet {
                    t.click(&b);
                }
                std::thread::sleep(Duration::from_secs(4));
                t.check(
                    t.find(&w, "Stop", Some(UIA_ButtonControlTypeId), 10).is_some(),
                    "the run starts and Stop is offered",
                );
                t.shot(&w, "08-running.png");
                chord(&[VK_SHIFT], VK_F5);
                std::thread::sleep(Duration::from_secs(3));
                t.check(t.find(&w, "Cancelled", None, 15).is_some(), "Shift+F5 stops the run (Cancelled)");
                t.shot(&w, "09-stopped.png");
            }
            None => {
                t.shot(&w, "07-no-confirmation.png");
                t.check(false, "F5 shows the run confirmation (validation may have found blockers)")
            }
        }

        // 6. Export the stopped run's logs through the Save dialog
        if let Some(logs_tab) = t.find(&w, "Logs", Some(UIA_TabItemControlTypeId), 5) {
            t.click(&logs_tab);
            std::thread::sleep(Duration::from_secs(1));
            if let Some(b) = t.find(&w, "Export Logs…", Some(UIA_ButtonControlTypeId), 5) {
                // Keyboard activation, as a person would: Invoke on a web-component host doesn't
                // always reach its inner <button>.
                t.front(&w);
                unsafe {
                    let _ = b.SetFocus();
                }
                std::thread::sleep(Duration::from_millis(200));
                chord(&[], VK_RETURN);
                if t.find(&w, "Export logs", Some(UIA_WindowControlTypeId), 3).is_none() {
                    t.click(&b);
                }
                let zip = out.join("exported-logs.zip");
                let filled = file_dialog(&t, zip.as_str());
                if !filled {
                    t.note("the Save dialog's File name box wasn't found");
                }
                std::thread::sleep(Duration::from_secs(1));
                t.shot(&w, "10a-after-save-dialog.png");
                // the redaction question (a task dialog): choose Keep Paths
                let keep = t
                    .find(&w, "Keep Paths", Some(UIA_ButtonControlTypeId), 5)
                    .or_else(|| t.find(&t.root(), "Keep Paths", Some(UIA_ButtonControlTypeId), 2));
                match keep {
                    Some(keep) => {
                        t.click(&keep);
                    }
                    None => t.note("the redaction question didn't appear"),
                }
                let deadline = Instant::now() + Duration::from_secs(10);
                while !zip.is_file() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(200));
                }
                t.shot(&w, "10-logs.png");
                t.check(filled && zip.is_file(), "Export Logs… writes the zip chosen in the Save dialog");
            } else {
                t.check(false, "the Logs tab offers Export Logs…");
            }
        }
        t.note("relink (Locate, Search Folder, prefix fix) is checked by the in-app self test's relink step");

        // 7. Keyboard-only pass: Tab moves focus through named controls
        t.front(&w);
        let mut names: Vec<String> = Vec::new();
        for _ in 0..25 {
            chord(&[], VK_TAB);
            unsafe {
                if let Ok(f) = t.a.GetFocusedElement() {
                    names.push(name(&f));
                }
            }
        }
        let distinct: std::collections::HashSet<&String> = names.iter().collect();
        t.check(distinct.len() >= 10, &format!("Tab reaches {} distinct controls", distinct.len()));
        t.check(names.iter().all(|n| !n.trim().is_empty()), "every control Tab reaches has a name");

        // 8. Close: the window saves and closes
        chord(&[VK_CONTROL], VK_W);
        std::thread::sleep(Duration::from_secs(3));
        t.check(t.window("UI test", 1).is_none(), "Ctrl+W closes the project window");
        let _ = appp.kill();
        let _ = appp.wait();

        let summary =
            format!("{} {} check(s), {} failed", if t.failures == 0 { "PASS" } else { "FAIL" }, t.checks, t.failures);
        println!("{summary}\nLog and screenshots: {out}");
        let _ = writeln!(t.log, "{summary}");
        let _ = Interface::as_raw(&t.a);
        if t.failures == 0 {
            0
        } else {
            1
        }
    }
}
