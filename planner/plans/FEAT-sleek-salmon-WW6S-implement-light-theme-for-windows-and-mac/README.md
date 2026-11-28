---
id: FEAT-sleek-salmon-WW6S
type: feature
title: Implement light theme for Windows and Mac
status: proposed
implementation: none
approved: no
priority: medium
created: 2026-11-24
updated: 2026-11-24
attachments: []
---

# Implement light theme for Windows and Mac

## Summary

Both apps already follow the operating system's light or dark setting. There is no way to pick a theme inside the app, so a user whose OS is dark cannot have a light app (and the reverse). This feature adds an app-wide **Appearance** setting with three choices, **System** (default), **Light** and **Dark**, to Settings on macOS and Windows. The choice overrides the OS for the whole app: windows, panels, sheets, toasts, the spec editor, the spec preview, and Mermaid diagrams.

On macOS the existing preview-only setting (System / Light / Dark) stays, but gets a new default, **Same as the app**, so one choice drives everything unless the user forces the preview to a theme.

The light colors themselves are not redesigned. Light is today's light look; this feature makes it reachable and makes sure every surface renders correctly in it.

## Background (what exists today)

- **Windows**
  - `apps/windows/src/shell.rs` `theme(scale)` reads `AppsUseLightTheme` from the registry and returns `ThemeInfo { dark, high_contrast, accent, scale, mica }`.
  - `apps/windows/src/window.rs` `apply_frame_theme()` sets `DWMWA_USE_IMMERSIVE_DARK_MODE` and Mica from that value. `theme_changed()` re-reads it and pushes a `Topic::Theme` event when the OS changes.
  - `apps/windows/ui/src/theme.ts` `apply()` sets `data-theme` on `<html>`, builds the Fluent tokens (`webLightTheme` / `webDarkTheme`) and publishes the `theme` signal. `project/preview.ts` passes `theme.value.dark` to the preview.
  - `apps/windows/ui/src/styles/app.css` holds the light values in `:root` and the dark overrides in `[data-theme="dark"]`.
  - The Windows Settings pane `editor` text says "The editor follows the Windows theme".
- **macOS**
  - The app uses the system appearance (`NSApp.effectiveAppearance`). Colors resolve dynamically (for example `SyntaxHighlighter`, `IslandEditing`).
  - `AppSettings.previewTheme` (`UserDefaults`, values `system` | `light` | `dark`, shown in Settings › Editor › Preview › Appearance) picks only the spec preview's theme through `PreviewController.theme(for:)`.
  - `InlineDisplay.isDark` and `InlineRenderService` take the dark flag from the text view's effective appearance.
  - `SelfTest.swift` sets `NSApp.appearance = .aqua` for the light snapshots and resets it to `nil`.
- **Shared**
  - `crates/video-studio-core/src/settings.rs` `Settings` is saved as `<library>/settings.json`. Windows reads it directly; macOS mirrors it in `apps/macos/Sources/VideoStudio/Core/Models.swift` (`app.settings`) and saves it through `settings.save`. Unknown or missing fields must be tolerated (existing test), so a new field is backward compatible.

## Requirements

### Setting and storage

- [ ] `Settings` in `crates/video-studio-core/src/settings.rs` gets a field `appearance` of an enum `Appearance { System, Light, Dark }`, serialized as the strings `"system"`, `"light"`, `"dark"`. The default is `System`.
- [ ] A `settings.json` with no `appearance` key, or an unknown value (for example `"auto"`), loads as `System` without an error and without discarding other fields.
- [ ] Saving from either platform keeps the value, so a `settings.json` copied between a Mac and a Windows machine gives the same choice on both.
- [ ] The macOS `Settings` mirror in `Core/Models.swift` gets the same field with the same strings, and keeps any value it does not recognize on save.
- [ ] The Windows bridge types are regenerated (`apps/windows/ui/src/generated/*`) so the TypeScript `Settings` type includes `appearance`. No hand edits to generated files.

### macOS behavior

- [ ] Settings › General (or the top of Settings › Editor, whichever the implementer picks and records in `docs/decisions.md`) has a segmented or pop-up control labeled "Appearance" with the options "System", "Light" and "Dark".
- [ ] Choosing an option takes effect immediately in every open window, without restart: `System` sets `NSApp.appearance = nil`, `Light` sets `NSAppearance(named: .aqua)`, `Dark` sets `NSAppearance(named: .darkAqua)`.
- [ ] The saved choice is applied at launch before the first window is shown, so there is no flash of the wrong theme.
- [ ] With `System`, changing the macOS appearance while the app runs still switches the app, as today.
- [ ] Editor syntax colors, island tints, the table toolbar, completion popups, the inline Mermaid and math rendering (`InlineDisplay`, `InlineRenderService`) and the Claude panel re-render in the new appearance when it changes, including text that is already on screen.
- [ ] The preview setting in Settings › Editor › Preview › Appearance has the options "Same as the app", "Light" and "Dark". The stored value for "Same as the app" is `app` and it is the default.
- [ ] An existing stored `previewTheme` of `system` is read as `app`. `light` and `dark` keep their meaning. No other migration is needed.
- [ ] `PreviewController.theme(for:)` returns the app's effective theme for `app`, and `light` or `dark` for the forced values. Changing the app appearance re-renders open previews.
- [ ] `SelfTest.swift` and `DocShots.swift` no longer reset `NSApp.appearance` to `nil` when they finish. They restore the value the setting gives, so a run with a forced theme does not leak into later steps.

### Windows behavior

- [ ] Settings has an "Appearance" control with the options "System", "Light" and "Dark" (a Fluent radio group or select), placed in the `general` pane. It is reachable by keyboard and has an accessible name.
- [ ] `shell::theme()` takes the appearance setting into account: for `Light` it returns `dark: false`, for `Dark` it returns `dark: true`, and for `System` it keeps reading `AppsUseLightTheme`. `accent`, `scale`, `mica` and `high_contrast` are still read from the OS.
- [ ] High contrast wins. While a Windows high-contrast theme is active, the Appearance setting is ignored, the control is disabled, and a one-line note says why. The existing `.high-contrast` CSS and `forced-colors` handling are unchanged.
- [ ] Changing the setting updates every open window without restart: the web UI (`theme.ts` `apply()`), the title bar (`DWMWA_USE_IMMERSIVE_DARK_MODE` through `apply_frame_theme()`), and the Mica fallback background (`html[data-mica="false"]`).
- [ ] The setting change reaches all windows, including windows that were already open, through the existing `settings_changed` and `Topic::Theme` events. The host re-sends `ThemeInfo` after the setting is saved.
- [ ] With a forced `Light` or `Dark`, a later OS theme change (`WM_SETTINGCHANGE` / `ImmersiveColorSet`) does not flip the app. Accent color, contrast and DPI changes still update it.
- [ ] The WebView2 `PreferredColorScheme` is set to match the effective theme (`System`, `Light` or `Dark`) so CSS `prefers-color-scheme` queries inside the web content, including the preview's Mermaid, KaTeX and highlight.js, agree with the app.
- [ ] The saved choice is applied before the first window is shown, so there is no flash of the wrong theme at start-up.
- [ ] The preview already reads `theme.value.dark`, so it follows the app. No separate preview setting is added on Windows. The Settings text in the `editor` pane is updated to say the editor follows the app's Appearance setting and text scaling.

### Light theme quality

- [ ] On Windows, every surface is checked in forced Light on a dark OS and in forced Dark on a light OS: toolbar, sidebar, editor, preview, inspector tabs, App Library window, Settings, onboarding, sheets and dialogs (`dialog.sheet`), toasts, validation and run panels, log viewer, relink sheet and Claude panel. Any hard-coded color that only works in one theme is replaced by a token or by a `[data-theme]` rule in `apps/windows/ui/src/styles/app.css`.
- [ ] On macOS, the same pass is done in forced Light on a dark OS and forced Dark on a light OS. Any hard-coded `NSColor` or SwiftUI color that does not adapt is replaced by a dynamic color.
- [ ] Text and essential icons meet WCAG AA contrast (4.5:1 for body text, 3:1 for large text and icons) against their background in Light on both platforms, including the status colors (`--ok`, `--warn`, `--danger` and the macOS equivalents), badges and disabled controls.
- [ ] Mermaid diagrams, code blocks and tables in the spec preview and in the inline editor rendering are readable in Light and in Dark, and switch when the theme changes without reopening the document.
- [ ] Nothing in the images or video previews shown by the app (media thumbnails, output player) changes with the theme.

### Docs and tests

- [ ] `docs/video-studio/README.md` describes the Appearance setting for macOS and Windows (where it is, what the three choices do, that high contrast overrides it on Windows, and the macOS preview option).
- [ ] The decisions about the control's placement on macOS and the migration of `previewTheme` are recorded in `docs/decisions.md`.

## Acceptance criteria

- [ ] **Core:** a unit test in `settings.rs` covers the default (`System`), a round trip for each of the three values, a missing key, and an unknown value falling back to `System` with other fields preserved. `cargo test`, `cargo clippy -- -D warnings` and `cargo fmt --check` pass.
- [ ] **macOS, set and persist:** in the app, selecting Light, then Dark, then System changes `NSApp.effectiveAppearance` each time, the editor and inspector recolor immediately, and after quitting and relaunching the last choice is applied before the window appears.
- [ ] **macOS, override:** with the OS in dark mode and Appearance set to Light, the project window, Settings, the App Library window, sheets, the spec preview and an inline Mermaid diagram are all light. With the OS in light mode and Appearance set to Dark, all of them are dark.
- [ ] **macOS, preview:** with Appearance = Dark and Preview = "Same as the app" the preview is dark; changing Preview to Light makes only the preview light; a stored `previewTheme` of `system` appears as "Same as the app".
- [ ] **macOS self test:** the in-app self test (`VIDEO_STUDIO_SELFTEST`) takes snapshots in forced Light and forced Dark for the validation, preview and run views, and still ends with the appearance restored to the setting. It passes.
- [ ] **Windows, set and persist:** selecting Light, Dark and System in Settings changes `data-theme` on `<html>`, the Fluent tokens and the title bar immediately; after restart the last choice is applied before the window is shown.
- [ ] **Windows, override:** with Windows in dark mode and Appearance = Light, the app is light, including the title bar; toggling the Windows theme while the app runs does not change it. With Appearance = System the app follows the Windows theme as before.
- [ ] **Windows, high contrast:** with a high-contrast theme on, the control is disabled with the explanation, and the app uses the system colors regardless of the saved value. Turning high contrast off restores the saved choice.
- [ ] **Windows, multi-window:** with two project windows open, changing the setting in one updates both within one second.
- [ ] **Windows, tests:** the Windows in-app self test (`build.ps1 selftest`) snapshots the main window in forced Light and forced Dark. The UI Automation suite (`build.ps1 uitest`) selects each Appearance option by keyboard, checks that the control has an accessible name, and checks the high-contrast disabled state. These pass at 100 %, 150 % and 200 % scaling.
- [ ] **Cross-platform:** a `settings.json` written by the macOS app with `"appearance": "light"` opens on Windows as Light, and the reverse, with no other field lost.
- [ ] **Contrast:** a documented check (screenshot review or an automated contrast script) shows the AA ratios above for the main text, status colors and badges in Light on both platforms. Any failure is fixed, or listed in the implementation log as a known issue.
- [ ] The macOS CI job and the macOS self test still pass after the shared `settings.rs` change.

## Design notes

- **Where the value lives.** One shared field in `Settings` (`settings.json`) rather than a macOS-only `UserDefaults` key, because the Mac app already round-trips `Settings` through `settings.save` and the two platforms are meant to share the file. The alternative (a macOS `UserDefaults` key like `previewTheme`) is simpler on the Mac but would make the choice differ between platforms for no reason.
- **Spec impact.** This changes what `ui/spec.md` describes: §17 ("Full light and dark mode", follows the system), §18 Settings (General: add Appearance) and §23.6 (Themes and scaling: Windows follows the system theme). Per the Planner rules these are not edited here; they should be updated when the feature is implemented and the user asks.
- **Windows mechanics.** The cleanest change is to give `shell::theme()` the effective appearance and have `Window::apply_frame_theme()` and `theme_changed()` pass it through, so the frame and the web UI always agree. `ThemeInfo` does not need a new field if `dark` already carries the effective value; adding an `appearance` field for the Settings control to read is optional.
- **Out of scope.** Operating-system chrome the app does not draw: Windows toast notifications, the common file dialogs and the Win32 menu bar follow the OS theme, and macOS system alerts and open/save panels follow `NSApp.appearance` only where AppKit supports it. A new light palette, a "schedule" mode (light by day, dark by night), per-project themes, and theming the website under `docs/video-studio/site` are not part of this feature.
- **Edge cases.** Missing or corrupt `settings.json` loads defaults (System). A forced theme on macOS must also reach windows opened later, because `NSApp.appearance` is app-wide. On Windows the setting must reach a window whose WebView is still loading, so the host sends `ThemeInfo` in the initial bridge message (`bridge.rs`) as it does now.

## Implementation log
