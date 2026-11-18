import AppKit

#if DEBUG
SelfTest.prepareEnvironment()
DocShots.prepareEnvironment()
#endif

MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    app.delegate = delegate
    withExtendedLifetime(delegate) {
        _ = NSApplicationMain(CommandLine.argc, CommandLine.unsafeArgv)
    }
}
