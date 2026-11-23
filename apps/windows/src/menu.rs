//! The Win32 menu bar of a project window (ui/spec.md §23.3, §23.6). Choosing an item sends a
//! `menu` event with the command's name; the web UI performs it, and handles the same shortcuts
//! itself while it has the keyboard focus (WebView2 receives the keys first).

use windows::core::HSTRING;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, HMENU, MF_POPUP, MF_SEPARATOR, MF_STRING,
};

/// (command, label with its Windows shortcut). An empty command is a separator.
type Item = (&'static str, &'static str);

const FILE: &[Item] = &[
    ("new_project", "&New Project…\tCtrl+N"),
    ("open_project", "&Open…\tCtrl+O"),
    ("save", "&Save\tCtrl+S"),
    ("save_as", "Save &As…\tCtrl+Shift+S"),
    ("duplicate", "&Duplicate…"),
    ("save_as_template", "Save as &Template…"),
    ("export_spec", "&Export Spec…"),
    ("", ""),
    ("add_media", "Add &Media…\tCtrl+Shift+M"),
    ("cleanup_pasted", "Clean Up &Pasted Images…"),
    ("", ""),
    ("settings", "Se&ttings…\tCtrl+,"),
    ("", ""),
    ("close", "&Close\tCtrl+W"),
    ("quit", "E&xit"),
];

const EDIT: &[Item] = &[
    ("undo", "&Undo\tCtrl+Z"),
    ("redo", "&Redo\tCtrl+Y"),
    ("", ""),
    ("cut", "Cu&t\tCtrl+X"),
    ("copy", "&Copy\tCtrl+C"),
    ("paste", "&Paste\tCtrl+V"),
    ("select_all", "Select &All\tCtrl+A"),
    ("", ""),
    ("find", "&Find…\tCtrl+F"),
    ("replace", "R&eplace…\tCtrl+H"),
];

const VIEW: &[Item] = &[
    ("toggle_preview", "Toggle &Source / Preview\tCtrl+/"),
    ("", ""),
    ("sidebar_outline", "&Outline\tCtrl+1"),
    ("sidebar_media", "&Media\tCtrl+2"),
    ("sidebar_runs", "&Runs\tCtrl+3"),
    ("", ""),
    ("inspector_claude", "&Claude\tCtrl+Alt+1"),
    ("inspector_project", "&Project\tCtrl+Alt+2"),
    ("inspector_outputs", "O&utputs\tCtrl+Alt+3"),
    ("inspector_validation", "&Validation\tCtrl+Alt+4"),
    ("inspector_run", "R&un\tCtrl+Alt+5"),
];

const PROJECT: &[Item] = &[
    ("validate", "&Validate\tCtrl+Shift+B"),
    ("run", "&Run\tF5"),
    ("audio_review", "&Audio Review…\tCtrl+F5"),
    ("stop", "&Stop\tShift+F5"),
    ("", ""),
    ("relink", "Re&link Media…"),
    ("gather_info", "&Gather Missing Info"),
    ("send_to_claude", "Send to &Claude\tCtrl+Enter"),
];

const WINDOW: &[Item] = &[("library", "App &Library\tCtrl+Shift+L")];

const HELP: &[Item] = &[("help", "Syncfusion Video Studio &Help"), ("about", "&About Syncfusion Video Studio")];

const MENUS: &[(&str, &[Item])] =
    &[("&File", FILE), ("&Edit", EDIT), ("&View", VIEW), ("&Project", PROJECT), ("&Window", WINDOW), ("&Help", HELP)];

fn all_commands() -> impl Iterator<Item = &'static str> {
    MENUS.iter().flat_map(|(_, items)| items.iter().map(|(c, _)| *c)).filter(|c| !c.is_empty())
}

const FIRST_ID: u16 = 1000;

/// The command a menu item ID stands for.
pub fn command_name(id: u16) -> Option<&'static str> {
    id.checked_sub(FIRST_ID).and_then(|i| all_commands().nth(i as usize))
}

pub fn project_menu() -> HMENU {
    unsafe {
        let bar = CreateMenu().expect("menu bar");
        let mut next = FIRST_ID;
        for (title, items) in MENUS {
            let popup = CreatePopupMenu().expect("menu");
            for (cmd, label) in *items {
                if cmd.is_empty() {
                    let _ = AppendMenuW(popup, MF_SEPARATOR, 0, None);
                } else {
                    let _ = AppendMenuW(popup, MF_STRING, next as usize, &HSTRING::from(*label));
                    next += 1;
                }
            }
            let _ = AppendMenuW(bar, MF_POPUP, popup.0 as usize, &HSTRING::from(*title));
        }
        bar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_map_to_commands_and_every_spec_shortcut_is_listed() {
        assert_eq!(command_name(FIRST_ID), Some("new_project"));
        assert_eq!(command_name(FIRST_ID - 1), None);
        let labels: Vec<&str> = MENUS.iter().flat_map(|(_, i)| i.iter().map(|(_, l)| *l)).collect();
        // ui/spec.md §23.6
        for shortcut in ["Ctrl+/", "Ctrl+Shift+B", "\tF5", "Shift+F5", "Ctrl+Shift+L", "Ctrl+Enter", "Ctrl+,"] {
            assert!(labels.iter().any(|l| l.contains(shortcut)), "{shortcut}");
        }
        let names: Vec<&str> = all_commands().collect();
        let unique: std::collections::HashSet<&&str> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "command names are unique");
    }
}
