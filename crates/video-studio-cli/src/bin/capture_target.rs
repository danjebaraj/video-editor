//! `vs-capture-target`: the small Win32 app the Windows recording backend is tested against
//! (ui/spec.md §23.7). One window, "VS Capture Target", with a button, a text box, a menu
//! (File › Say Hello, Sound › Play Tone / Stop Tone) and a 440 Hz tone generator. Everything that
//! happens is appended to the file named by `VS_CAPTURE_TARGET_LOG`, so tests can check that a
//! click, typing or a menu choice really arrived. `--tone` starts the tone at launch.

#[cfg(not(windows))]
fn main() {
    eprintln!("vs-capture-target is a Windows test app");
}

#[cfg(windows)]
fn main() {
    target::run();
}

#[cfg(windows)]
mod target {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH};
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_LOOP, SND_MEMORY, SND_PURGE};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const ID_BUTTON: usize = 101;
    const ID_EDIT: usize = 102;
    const ID_HELLO: usize = 201;
    const ID_TONE: usize = 202;
    const ID_STOP: usize = 203;
    static LABEL: AtomicIsize = AtomicIsize::new(0);
    static EDIT: AtomicIsize = AtomicIsize::new(0);
    static TONE_ON: AtomicBool = AtomicBool::new(false);

    fn log(line: &str) {
        if let Some(p) = std::env::var_os("VS_CAPTURE_TARGET_LOG") {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    /// A one-second 440 Hz sine, 48 kHz mono 16-bit, as an in-memory WAV that loops seamlessly.
    fn tone() -> &'static [u8] {
        static WAV: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        WAV.get_or_init(|| {
            let rate = 48000u32;
            let samples: Vec<i16> = (0..rate)
                .map(|i| ((i as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin() * 12000.0) as i16)
                .collect();
            let data_len = (samples.len() * 2) as u32;
            let mut v = Vec::new();
            v.extend_from_slice(b"RIFF");
            v.extend_from_slice(&(36 + data_len).to_le_bytes());
            v.extend_from_slice(b"WAVEfmt ");
            v.extend_from_slice(&16u32.to_le_bytes());
            v.extend_from_slice(&1u16.to_le_bytes());
            v.extend_from_slice(&1u16.to_le_bytes());
            v.extend_from_slice(&rate.to_le_bytes());
            v.extend_from_slice(&(rate * 2).to_le_bytes());
            v.extend_from_slice(&2u16.to_le_bytes());
            v.extend_from_slice(&16u16.to_le_bytes());
            v.extend_from_slice(b"data");
            v.extend_from_slice(&data_len.to_le_bytes());
            for s in samples {
                v.extend_from_slice(&s.to_le_bytes());
            }
            v
        })
    }

    fn set_tone(on: bool) {
        unsafe {
            if on {
                let _ = PlaySoundW(PCWSTR(tone().as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_LOOP);
            } else {
                let _ = PlaySoundW(PCWSTR::null(), None, SND_PURGE);
            }
        }
        TONE_ON.store(on, Ordering::SeqCst);
        log(if on { "tone on" } else { "tone off" });
    }

    fn set_label(text: &str) {
        let h = HWND(LABEL.load(Ordering::SeqCst) as _);
        unsafe {
            let _ = SetWindowTextW(h, &windows::core::HSTRING::from(text));
        }
    }

    extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_COMMAND => {
                    let id = wp.0 & 0xffff;
                    let code = (wp.0 >> 16) & 0xffff;
                    match id {
                        ID_BUTTON => {
                            log("button clicked");
                            set_label("Pressed");
                        }
                        ID_EDIT if code == EN_CHANGE as usize => {
                            let mut buf = [0u16; 512];
                            let n = GetWindowTextW(HWND(EDIT.load(Ordering::SeqCst) as _), &mut buf) as usize;
                            log(&format!("text: {}", String::from_utf16_lossy(&buf[..n])));
                        }
                        ID_HELLO => {
                            log("menu: Say Hello");
                            set_label("Hello from the menu");
                        }
                        ID_TONE => set_tone(true),
                        ID_STOP => set_tone(false),
                        _ => {}
                    }
                    LRESULT(0)
                }
                WM_KEYDOWN => {
                    log(&format!("key: {}", wp.0));
                    LRESULT(0)
                }
                WM_MOUSEWHEEL => {
                    log(&format!("wheel: {}", ((wp.0 >> 16) & 0xffff) as i16));
                    LRESULT(0)
                }
                WM_DESTROY => {
                    set_tone(false);
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    pub fn run() {
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: inst.into(),
                lpszClassName: w!("VsCaptureTarget"),
                hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&class);
            let menu = CreateMenu().unwrap();
            let file = CreatePopupMenu().unwrap();
            let _ = AppendMenuW(file, MF_STRING, ID_HELLO, w!("Say &Hello"));
            let sound = CreatePopupMenu().unwrap();
            let _ = AppendMenuW(sound, MF_STRING, ID_TONE, w!("&Play Tone"));
            let _ = AppendMenuW(sound, MF_STRING, ID_STOP, w!("&Stop Tone"));
            let _ = AppendMenuW(menu, MF_POPUP, file.0 as usize, w!("&File"));
            let _ = AppendMenuW(menu, MF_POPUP, sound.0 as usize, w!("&Sound"));
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("VsCaptureTarget"),
                w!("VS Capture Target"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                120,
                120,
                640,
                420,
                None,
                Some(menu),
                Some(inst.into()),
                None,
            )
            .unwrap();
            let button = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                w!("Press me"),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                20,
                20,
                160,
                40,
                Some(hwnd),
                Some(HMENU(ID_BUTTON as _)),
                Some(inst.into()),
                None,
            )
            .unwrap();
            let edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                20,
                80,
                400,
                30,
                Some(hwnd),
                Some(HMENU(ID_EDIT as _)),
                Some(inst.into()),
                None,
            )
            .unwrap();
            let label = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Ready"),
                WS_CHILD | WS_VISIBLE,
                20,
                130,
                400,
                30,
                Some(hwnd),
                None,
                Some(inst.into()),
                None,
            )
            .unwrap();
            let _ = button;
            EDIT.store(edit.0 as isize, Ordering::SeqCst);
            LABEL.store(label.0 as isize, Ordering::SeqCst);
            log("started");
            if std::env::args().any(|a| a == "--tone") {
                set_tone(true);
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
