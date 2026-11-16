import SwiftUI

/// The controls for a finish: how shots join, the beat, and how the music comes and goes.
/// Used for the app-wide default (Settings › Finish) and for a project's own finish.
struct FinishEditor: View {
    @Binding var finish: Finish
    /// Narrow layout for the project inspector.
    var compact = false

    var body: some View {
        VStack(alignment: .leading, spacing: compact ? 8 : 12) {
            Picker("Between scenes", selection: $finish.transitions) {
                Text("Smooth").tag("smooth")
                Text("Plain cuts").tag("cuts")
                Text("Lively").tag("lively")
            }
            .pickerStyle(.segmented)
            .labelsHidden(compact)
            .accessibilityLabel("Transitions between scenes")
            .help("Smooth: a short soft move between scenes and plain cuts inside a scene. Plain cuts: no transitions except a fade for the title and end card. Lively: quick moving transitions, for social cuts.")
            Text(transitionNote).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            if finish.transitions != "cuts" {
                seconds("Transition length", $finish.transition_s, 0.15...1.5, step: 0.05,
                        help: "How long a transition between two scenes takes. 0.3 to 0.5 seconds reads as smooth without slowing the video.")
            }
            Toggle(isOn: $finish.cut_on_beat) { wrapping("Cut on the music's beat") }
                .help("Cuts timed to the narration move onto the nearest beat, scene changes go to the start of a bar where one is close, and generated music is asked for with a steady beat that can be followed.")
            Toggle(isOn: $finish.ease_text) { wrapping("Ease on-screen text in and out") }
                .help("Text fades or slides in shortly after a cut and leaves before the next one, instead of popping on.")
            Toggle(isOn: $finish.guide_the_eye) { wrapping(compact ? "Ease into what is being talked about" : "Ease into the part of the screen being talked about") }
                .help("On screen recordings, a gentle zoom toward what the narration is describing, and back out before the cut.")
            Divider()
            seconds("Before the first word", $finish.lead_in_s, 0...5, step: 0.1,
                    help: "Picture and music start first; the voice comes in after this long. About a second keeps the video from starting on a cold word.")
            seconds("After the last word", $finish.tail_s, 0...8, step: 0.25,
                    help: "How long the end card holds after the voice stops, so the video doesn't end on the last syllable.")
            seconds("Music fades in over", $finish.music_fade_in_s, 0...5, step: 0.25,
                    help: "How long the music takes to come in at the start.")
            seconds("Music fades out over", $finish.music_fade_out_s, 0...8, step: 0.25,
                    help: "Used when the track has no ending of its own to land on the end of the video. A track with a real ending closes the video on it instead.")
        }
    }

    /// Labels wrap instead of pushing the narrow inspector wider than its column.
    private func wrapping(_ text: String) -> some View {
        Text(text).fixedSize(horizontal: false, vertical: true)
    }

    private var transitionNote: String {
        switch finish.transitions {
        case "cuts": return "Every scene change is a plain cut. The title and the end card still fade."
        case "lively": return "Scene changes use quick moving transitions (whips and pushes). Best for short social videos."
        default: return "Scene changes use a short soft move; views of the same screen are joined with plain cuts, so text never ghosts."
        }
    }

    private func seconds(_ title: String, _ value: Binding<Double>, _ range: ClosedRange<Double>, step: Double, help: String) -> some View {
        HStack(spacing: 8) {
            Text(title).lineLimit(2).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 4)
            Text(Finish.seconds(value.wrappedValue)).monospacedDigit().foregroundStyle(.secondary)
            Stepper("", value: value, in: range, step: step).labelsHidden().accessibilityLabel(title)
        }
        .help(help)
    }
}

/// Settings › Finish: the finish every project follows unless it keeps its own.
struct FinishSettings: View {
    @ObservedObject private var app = AppState.shared

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("How every video is finished")
                .font(.headline)
            Text("Each run applies these choices on top of the spec: how scenes join, whether cuts follow the music, and how the music comes in and goes out. A project follows them unless it is given its own under Project › Finish. A spec can still ask for something different in words.")
                .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            FinishEditor(finish: $app.settings.finish)
                .frame(maxWidth: 460, alignment: .leading)
            HStack {
                Button("Restore Defaults") { app.settings.finish = Finish() }
                    .disabled(app.settings.finish == Finish())
                    .help("Smooth transitions of 0.4 s, cuts on the beat, a short lead-in, and gentle music fades")
                Spacer()
            }
        }
    }
}

/// Project › Finish: follow Settings, or keep this project's own choices.
struct FinishSection: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject private var app = AppState.shared

    private var own: Bool { document.project.finish != nil }

    var body: some View {
        InspectorSection(title: "Finish") {
            Picker("Finish", selection: Binding(get: { own }, set: { useOwn in
                // a project's own finish starts as a copy of the app's, so nothing changes until edited
                document.update { $0.finish = useOwn ? app.settings.finish : nil }
            })) {
                Text("Settings default").tag(false)
                Text("This project's own").tag(true)
            }
            .labelsHidden()
            .accessibilityLabel("Finish")
            .help("Transitions, cuts on the beat and music fades. Settings default follows Settings › Finish, including later changes to it.")
            if own {
                FinishEditor(finish: Binding(get: { document.project.finish ?? app.settings.finish },
                                             set: { v in document.update { $0.finish = v } }), compact: true)
                Button("Apply the Settings Default") { document.update { $0.finish = app.settings.finish } }
                    .disabled(document.project.finish == app.settings.finish)
                    .help("Replace this project's finish with a copy of the one in Settings › Finish")
            } else {
                Text(app.settings.finish.summary + " Change it in Settings › Finish.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

private extension View {
    @ViewBuilder func labelsHidden(_ hidden: Bool) -> some View {
        if hidden { self.labelsHidden() } else { self }
    }
}
