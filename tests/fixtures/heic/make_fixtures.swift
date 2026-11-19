import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers
// usage: make <out> <w> <h> <orientation> <quality>
let a = CommandLine.arguments
let out = URL(fileURLWithPath: a[1]); let w = Int(a[2])!, h = Int(a[3])!; let orient = Int(a[4])!; let q = Double(a[5])!
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: cs, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
// CG origin bottom-left; draw quadrants so stored top-left = red, top-right = green, bottom-left = blue, bottom-right = yellow
func fill(_ r: CGFloat,_ g: CGFloat,_ b: CGFloat,_ x: Int,_ y: Int,_ ww: Int,_ hh: Int){ ctx.setFillColor(red:r,green:g,blue:b,alpha:1); ctx.fill(CGRect(x:x,y:y,width:ww,height:hh)) }
let hw = w/2, hh = h/2
fill(0.9,0.1,0.1, 0, hh, hw, h-hh)      // top-left red
fill(0.1,0.8,0.2, hw, hh, w-hw, h-hh)   // top-right green
fill(0.1,0.2,0.9, 0, 0, hw, hh)         // bottom-left blue
fill(0.95,0.85,0.1, hw, 0, w-hw, hh)    // bottom-right yellow
// a gradient band and grid lines so tiles misplaced would show
for i in stride(from: 0, to: w, by: 256) { fill(0,0,0, i, 0, 4, h) }
for j in stride(from: 0, to: h, by: 256) { fill(1,1,1, 0, j, w, 4) }
// white marker block near stored top-left
fill(1,1,1, 40, h-140, 100, 100)
let img = ctx.makeImage()!
let dest = CGImageDestinationCreateWithURL(out as CFURL, UTType.heic.identifier as CFString, 1, nil)!
var props: [CFString: Any] = [kCGImageDestinationLossyCompressionQuality: q]
if orient != 1 { props[kCGImagePropertyOrientation] = orient }
CGImageDestinationAddImage(dest, img, props as CFDictionary)
precondition(CGImageDestinationFinalize(dest))
