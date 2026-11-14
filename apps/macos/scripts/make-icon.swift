// Draws the app icon at every size macOS needs. Run: swift scripts/make-icon.swift
import AppKit

func draw(_ size: CGFloat) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let ctx = NSGraphicsContext.current!.cgContext
    let s = size / 1024
    // macOS icon grid: an 824 pt rounded square centred in 1024.
    let rect = CGRect(x: 100 * s, y: 100 * s, width: 824 * s, height: 824 * s)
    let path = CGPath(roundedRect: rect, cornerWidth: 185 * s, cornerHeight: 185 * s, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10 * s), blur: 24 * s, color: NSColor.black.withAlphaComponent(0.3).cgColor)
    ctx.addPath(path); ctx.setFillColor(NSColor(calibratedRed: 0.11, green: 0.13, blue: 0.2, alpha: 1).cgColor); ctx.fillPath()
    ctx.restoreGState()
    ctx.saveGState()
    ctx.addPath(path); ctx.clip()
    let colors = [NSColor(calibratedRed: 0.16, green: 0.2, blue: 0.33, alpha: 1).cgColor, NSColor(calibratedRed: 0.07, green: 0.09, blue: 0.16, alpha: 1).cgColor] as CFArray
    let grad = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1])!
    ctx.drawLinearGradient(grad, start: CGPoint(x: rect.midX, y: rect.maxY), end: CGPoint(x: rect.midX, y: rect.minY), options: [])
    // A document page: the spec.
    let page = CGRect(x: 250 * s, y: 235 * s, width: 400 * s, height: 554 * s)
    ctx.addPath(CGPath(roundedRect: page, cornerWidth: 34 * s, cornerHeight: 34 * s, transform: nil))
    ctx.setFillColor(NSColor(calibratedWhite: 0.97, alpha: 1).cgColor); ctx.fillPath()
    ctx.setFillColor(NSColor(calibratedWhite: 0.72, alpha: 1).cgColor)
    for (i, w) in [250.0, 300.0, 210.0, 280.0, 170.0].enumerated() {
        let y = page.maxY - CGFloat(96 + i * 62) * s
        ctx.addPath(CGPath(roundedRect: CGRect(x: page.minX + 50 * s, y: y, width: CGFloat(w) * s, height: 22 * s), cornerWidth: 11 * s, cornerHeight: 11 * s, transform: nil))
    }
    ctx.fillPath()
    // A play button: the video it becomes.
    let c = CGPoint(x: 640 * s, y: 380 * s)
    ctx.setShadow(offset: CGSize(width: 0, height: -8 * s), blur: 22 * s, color: NSColor.black.withAlphaComponent(0.35).cgColor)
    ctx.addEllipse(in: CGRect(x: c.x - 170 * s, y: c.y - 170 * s, width: 340 * s, height: 340 * s))
    ctx.setFillColor(NSColor(calibratedRed: 0.95, green: 0.42, blue: 0.13, alpha: 1).cgColor); ctx.fillPath()
    ctx.setShadow(offset: .zero, blur: 0, color: nil)
    let tri = CGMutablePath()
    tri.move(to: CGPoint(x: c.x - 52 * s, y: c.y + 86 * s)); tri.addLine(to: CGPoint(x: c.x + 96 * s, y: c.y)); tri.addLine(to: CGPoint(x: c.x - 52 * s, y: c.y - 86 * s)); tri.closeSubpath()
    ctx.addPath(tri); ctx.setFillColor(NSColor.white.cgColor); ctx.fillPath()
    ctx.restoreGState()
    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

let out = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "Resources/Assets.xcassets/AppIcon.appiconset")
for size in [16, 32, 64, 128, 256, 512, 1024] {
    try! draw(CGFloat(size)).write(to: out.appendingPathComponent("icon_\(size).png"))
}
print("icons written to \(out.path)")
