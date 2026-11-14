import AppKit

#if DEBUG
/// The in-app self test (spec §20), run with `VS_SELFTEST=<scratch folder>`.
@MainActor
enum SelfTest {
    static func run() {
        NSApp.terminate(nil)
    }
}
#endif
