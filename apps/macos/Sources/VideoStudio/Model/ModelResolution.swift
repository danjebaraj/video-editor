import Foundation

/// What a model is for (the core's `project::ModelUse`).
enum ModelUse: String {
    case assistant
    case dry_run
    /// Production runs and Audio Reviews.
    case production
}

/// The model the next operation uses, for display, with the same precedence as the core's
/// `project::effective_model` (FFI `models.effective`, BUG-fond-crane-62ZP). Pure, so it can be
/// checked without the app: an override for the active provider, then the project's (or run
/// config's) choice for that provider, then that provider's default in Settings, then its CLI
/// default. Claude always gets a model; for Codex `nil` means Codex's own default. Claude's choices
/// are never used for Codex and stay where they are for when Claude is selected again.
enum ModelResolution {
    static func effective(codex: Bool, override: String? = nil, own: String, settingsDefault: String, claudeFallback: String) -> String? {
        let given: (String?) -> String? = { s in
            let t = (s ?? "").trimmingCharacters(in: .whitespaces)
            return t.isEmpty ? nil : t
        }
        if let o = given(override) { return o }
        if codex { return given(own) ?? given(settingsDefault) }
        return given(own) ?? given(settingsDefault) ?? claudeFallback
    }

    /// The label a picker or header shows for an effective model: "Codex default" when Codex
    /// chooses, a Codex model's name from its list (else its ID), or Claude's display name.
    static func label(effective: String?, codex: Bool, codexModels: [(id: String, name: String)], claudeName: (String) -> String) -> String {
        guard let m = effective else { return codex ? "Codex default" : "Default" }
        if codex { return codexModels.first { $0.id == m }?.name ?? m }
        return claudeName(m)
    }

    /// The label of a Codex picker's empty choice: it follows Settings › Codex, which may itself be
    /// Codex's default.
    static func codexEmptyChoiceLabel(settingsCodexModel: String, codexModels: [(id: String, name: String)]) -> String {
        let s = settingsCodexModel.trimmingCharacters(in: .whitespaces)
        if s.isEmpty { return "Codex default" }
        return "Settings default (\(label(effective: s, codex: true, codexModels: codexModels, claudeName: { $0 })))"
    }
}
