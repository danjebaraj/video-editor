//! Native dialogs: the common item dialogs (`IFileOpenDialog`, `IFileSaveDialog`) for files and
//! folders, and a task dialog for questions with named buttons (Reload / Keep Mine / Compare).

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Controls::{
    TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TD_WARNING_ICON,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName,
    FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};

use crate::protocol::{ConfirmArgs, FileFilter, OpenDialogArgs, SaveDialogArgs};

struct Filters {
    _names: Vec<HSTRING>,
    _specs: Vec<HSTRING>,
    specs: Vec<COMDLG_FILTERSPEC>,
}

fn filters(list: &[FileFilter]) -> Filters {
    let names: Vec<HSTRING> = list.iter().map(|f| HSTRING::from(f.name.as_str())).collect();
    let specs_s: Vec<HSTRING> = list.iter().map(|f| HSTRING::from(f.spec.as_str())).collect();
    let specs = names
        .iter()
        .zip(specs_s.iter())
        .map(|(n, s)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(s.as_ptr()) })
        .collect();
    Filters { _names: names, _specs: specs_s, specs }
}

unsafe fn item_path(item: &IShellItem) -> Option<String> {
    let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

unsafe fn folder_item(path: &str) -> Option<IShellItem> {
    let dir = std::path::Path::new(path);
    let dir = if dir.is_dir() { dir } else { dir.parent()? };
    SHCreateItemFromParsingName(&HSTRING::from(dir.to_str()?), None).ok()
}

/// Files or folders chosen by the user; empty when cancelled.
pub fn open(owner: HWND, args: &OpenDialogArgs) -> Vec<String> {
    unsafe {
        let Ok(d) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else {
            return Vec::new();
        };
        let mut opts = d.GetOptions().unwrap_or_default() | FOS_FORCEFILESYSTEM;
        if args.multiple {
            opts |= FOS_ALLOWMULTISELECT;
        }
        if args.folders {
            opts |= FOS_PICKFOLDERS;
        }
        let _ = d.SetOptions(opts);
        if !args.title.is_empty() {
            let _ = d.SetTitle(&HSTRING::from(args.title.as_str()));
        }
        let f = filters(&args.filters);
        if !f.specs.is_empty() && !args.folders {
            let _ = d.SetFileTypes(&f.specs);
        }
        if let Some(item) = args.default_path.as_deref().and_then(|p| folder_item(p)) {
            let _ = d.SetFolder(&item);
        }
        if d.Show(Some(owner)).is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Ok(items) = d.GetResults() {
            let n = items.GetCount().unwrap_or(0);
            for i in 0..n {
                if let Ok(item) = items.GetItemAt(i) {
                    if let Some(p) = item_path(&item) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }
}

/// The file to save to, or `None` when cancelled.
pub fn save(owner: HWND, args: &SaveDialogArgs) -> Option<String> {
    unsafe {
        let d = CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = d.GetOptions().unwrap_or_default() | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT;
        let _ = d.SetOptions(opts);
        if !args.title.is_empty() {
            let _ = d.SetTitle(&HSTRING::from(args.title.as_str()));
        }
        let f = filters(&args.filters);
        if !f.specs.is_empty() {
            let _ = d.SetFileTypes(&f.specs);
            // the extension of the first filter is added when the user types none
            if let Some(ext) = args.filters[0].spec.split(';').next().and_then(|s| s.strip_prefix("*.")) {
                let _ = d.SetDefaultExtension(&HSTRING::from(ext));
            }
        }
        if let Some(name) = &args.default_name {
            let _ = d.SetFileName(&HSTRING::from(name.as_str()));
        }
        if let Some(item) = args.default_path.as_deref().and_then(|p| folder_item(p)) {
            let _ = d.SetFolder(&item);
        }
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        item_path(&item)
    }
}

/// Asks a question with named buttons; returns the index of the one chosen (the last, as
/// "Cancel", when the dialog is dismissed).
pub fn confirm(owner: HWND, args: &ConfirmArgs) -> usize {
    let title = HSTRING::from(args.title.as_str());
    let message = HSTRING::from(args.message.as_str());
    let labels: Vec<HSTRING> = args.buttons.iter().map(|b| HSTRING::from(b.as_str())).collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| TASKDIALOG_BUTTON { nButtonID: 100 + i as i32, pszButtonText: PCWSTR(l.as_ptr()) })
        .collect();
    let app = HSTRING::from("Syncfusion Video Studio");
    let mut config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: PCWSTR(app.as_ptr()),
        pszMainInstruction: PCWSTR(title.as_ptr()),
        pszContent: PCWSTR(message.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: 100,
        ..Default::default()
    };
    if args.warning {
        config.Anonymous1.pszMainIcon = TD_WARNING_ICON;
    }
    let mut chosen = 0i32;
    let ok = unsafe { TaskDialogIndirect(&config, Some(&mut chosen), None, None).is_ok() };
    if ok && chosen >= 100 && ((chosen - 100) as usize) < labels.len() {
        (chosen - 100) as usize
    } else {
        labels.len().saturating_sub(1)
    }
}
