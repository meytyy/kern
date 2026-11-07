// Derives the square app icon source from the original logo, which stays untouched.
// macOS frames icons with transparent margins in a gray plate, so the artwork is
// cropped to its rounded square and placed on an opaque background: the system
// applies its own icon mask. Usage: swift scripts/make_icon.swift logo.png out.png
import AppKit

let args = CommandLine.arguments
guard args.count == 3, let source = NSImage(contentsOfFile: args[1]),
      let cg = source.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    fatalError("usage: make_icon.swift logo.png out.png")
}
let width = cg.width, height = cg.height
var pixels = [UInt8](repeating: 0, count: width * height * 4)
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let reader = CGContext(data: &pixels, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                       space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
reader.draw(cg, in: CGRect(x: 0, y: 0, width: width, height: height))
var (minX, minY, maxX, maxY) = (width, height, 0, 0)
for y in 0..<height {
    for x in 0..<width where pixels[(y * width + x) * 4 + 3] > 200 {
        minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
    }
}
let size = 1024
let out = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4,
                    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
out.setFillColor(CGColor(red: 0.075, green: 0.075, blue: 0.09, alpha: 1))
out.fill(CGRect(x: 0, y: 0, width: size, height: size))
let scale = CGFloat(size) / CGFloat(max(maxX - minX, maxY - minY))
out.interpolationQuality = .high
out.draw(cg, in: CGRect(x: -CGFloat(minX) * scale, y: -CGFloat(height - maxY) * scale,
                        width: CGFloat(width) * scale, height: CGFloat(height) * scale))
let rep = NSBitmapImageRep(cgImage: out.makeImage()!)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
