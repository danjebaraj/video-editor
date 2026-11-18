// Reads every picture it is given (macOS Vision text recognition) and fails when one shows something
// that shouldn't be published: this Mac's user name, its home folder, a path under /Users that isn't
// /Users/Shared, or an email address. Used by docshots.sh before the pictures are copied into the docs.
//
//   swiftc -O -o scan-shots scan-shots.swift && ./scan-shots shots/*.png
import AppKit
import Foundation
import Vision

let user = NSUserName()
let home = NSHomeDirectory()
let emailPattern = try! NSRegularExpression(pattern: "[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+\\.[A-Za-z]{2,}")
let userPathPattern = try! NSRegularExpression(pattern: "/Users/(?!Shared)[A-Za-z0-9._-]+")

func recognizedText(_ url: URL) -> [String] {
    guard let image = NSImage(contentsOf: url), let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return [] }
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    try? VNImageRequestHandler(cgImage: cg, options: [:]).perform([request])
    return (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }
}

var problems = 0
for path in CommandLine.arguments.dropFirst() {
    let lines = recognizedText(URL(fileURLWithPath: path))
    for line in lines {
        let range = NSRange(line.startIndex..., in: line)
        var why: String?
        if !user.isEmpty, line.localizedCaseInsensitiveContains(user) { why = "the user name" }
        else if line.contains(home) { why = "the home folder" }
        else if userPathPattern.firstMatch(in: line, range: range) != nil { why = "a path under /Users" }
        else if emailPattern.firstMatch(in: line, range: range) != nil { why = "an email address" }
        if let why {
            print("✘ \((path as NSString).lastPathComponent): \(why): \"\(line)\"")
            problems += 1
        }
    }
}
if problems == 0 { print("✔ no personal details found in \(CommandLine.arguments.count - 1) pictures") }
exit(problems == 0 ? 0 : 1)
