// Builds the Windows icon (assets/windows/kern.ico) from the original logo, which
// stays untouched. Windows draws icons as they are, without the mask macOS applies,
// so unlike scripts/make_icon.swift the corners outside the rounded square stay
// fully transparent here.
//
//   swift scripts/make_win_icon.swift logo.png assets/windows/kern.ico [preview-dir]
//
// Sizes 16, 24, 32, 48, 64 and 128 are 32-bit BMP entries with alpha (every
// Windows component reads them); 256 is PNG, as Windows expects for that size.
import AppKit

let args = CommandLine.arguments
guard args.count >= 3, let source = NSImage(contentsOfFile: args[1]),
      let cg = source.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    fatalError("usage: make_win_icon.swift logo.png out.ico [preview-dir]")
}
let previewDir = args.count > 3 ? args[3] : nil
let space = CGColorSpace(name: CGColorSpace.sRGB)!

func rgba(_ image: CGImage) -> [UInt8] {
    var pixels = [UInt8](repeating: 0, count: image.width * image.height * 4)
    let ctx = CGContext(data: &pixels, width: image.width, height: image.height, bitsPerComponent: 8,
                        bytesPerRow: image.width * 4, space: space,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
    return pixels
}

// The rounded square: everything clearly opaque. A soft drop shadow or halo
// around it is left out, so nothing tints the desktop around the icon.
let (w, h) = (cg.width, cg.height)
var src = rgba(cg)
var (minX, minY, maxX, maxY) = (w, h, 0, 0)
for y in 0..<h {
    for x in 0..<w where src[(y * w + x) * 4 + 3] > 200 {
        minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
    }
}
// Faint pixels outside the shape's box (shadow) are cleared; the anti-aliased
// rounded edge inside it is kept.
for y in 0..<h {
    for x in 0..<w where x < minX || x > maxX || y < minY || y > maxY {
        let i = (y * w + x) * 4
        src[i] = 0; src[i + 1] = 0; src[i + 2] = 0; src[i + 3] = 0
    }
}
let cleaned = CGContext(data: &src, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                        space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!.makeImage()!
let shape = cleaned.cropping(to: CGRect(x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1))!

/// One rendition: the shape centred with equal margins. Small sizes keep a
/// thinner margin so the K stays as large as possible.
func render(_ size: Int) -> [UInt8] {
    let margin: CGFloat = size <= 32 ? 0.5 : CGFloat(size) * 0.0625
    let box = CGFloat(size) - 2 * margin
    let scale = box / CGFloat(max(shape.width, shape.height))
    let dw = CGFloat(shape.width) * scale, dh = CGFloat(shape.height) * scale
    // Halving steps first: a single jump from ~1100 px to 16 px loses the K.
    var image = shape
    while CGFloat(image.width) / 2 > dw * 1.5 {
        let nw = image.width / 2, nh = image.height / 2
        let c = CGContext(data: nil, width: nw, height: nh, bitsPerComponent: 8, bytesPerRow: nw * 4,
                          space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        c.interpolationQuality = .high
        c.draw(image, in: CGRect(x: 0, y: 0, width: nw, height: nh))
        image = c.makeImage()!
    }
    var out = [UInt8](repeating: 0, count: size * size * 4)
    let ctx = CGContext(data: &out, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4,
                        space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    ctx.draw(image, in: CGRect(x: (CGFloat(size) - dw) / 2, y: (CGFloat(size) - dh) / 2, width: dw, height: dh))
    return out // premultiplied RGBA, top row first
}

func unpremultiplied(_ p: [UInt8]) -> [UInt8] {
    var o = p
    for i in stride(from: 0, to: p.count, by: 4) {
        let a = Int(p[i + 3])
        if a == 0 { o[i] = 0; o[i + 1] = 0; o[i + 2] = 0; continue }
        for k in 0..<3 { o[i + k] = UInt8(min(255, (Int(p[i + k]) * 255 + a / 2) / a)) }
    }
    return o
}

func png(_ p: [UInt8], _ size: Int) -> Data {
    var copy = p
    let ctx = CGContext(data: &copy, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4,
                        space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    return NSBitmapImageRep(cgImage: ctx.makeImage()!).representation(using: .png, properties: [:])!
}

extension Data {
    mutating func le16(_ v: Int) { append(contentsOf: [UInt8(v & 255), UInt8((v >> 8) & 255)]) }
    mutating func le32(_ v: Int) { le16(v & 0xffff); le16((v >> 16) & 0xffff) }
}

/// A 32-bit DIB icon entry: BGRA rows bottom-up, then the 1-bit AND mask.
func bmp(_ p: [UInt8], _ size: Int) -> Data {
    let rgba = unpremultiplied(p)
    var d = Data()
    d.le32(40); d.le32(size); d.le32(size * 2); d.le16(1); d.le16(32)
    d.le32(0); d.le32(0); d.le32(0); d.le32(0); d.le32(0); d.le32(0)
    for y in (0..<size).reversed() {
        for x in 0..<size {
            let i = (y * size + x) * 4
            d.append(contentsOf: [rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]])
        }
    }
    let rowBytes = ((size + 31) / 32) * 4
    for y in (0..<size).reversed() {
        var row = [UInt8](repeating: 0, count: rowBytes)
        for x in 0..<size where rgba[(y * size + x) * 4 + 3] == 0 {
            row[x / 8] |= UInt8(0x80 >> (x % 8))
        }
        d.append(contentsOf: row)
    }
    return d
}

let sizes = [16, 24, 32, 48, 64, 128, 256]
var entries: [(Int, Data)] = []
for size in sizes {
    let pixels = render(size)
    entries.append((size, size == 256 ? png(pixels, size) : bmp(pixels, size)))
    if let dir = previewDir {
        try! png(pixels, size).write(to: URL(fileURLWithPath: "\(dir)/kern-\(size).png"))
    }
}
var ico = Data()
ico.le16(0); ico.le16(1); ico.le16(entries.count)
var offset = 6 + 16 * entries.count
for (size, data) in entries {
    ico.append(contentsOf: [UInt8(size % 256), UInt8(size % 256), 0, 0])
    ico.le16(1); ico.le16(32); ico.le32(data.count); ico.le32(offset)
    offset += data.count
}
for (_, data) in entries { ico.append(data) }
try! ico.write(to: URL(fileURLWithPath: args[2]))
print("wrote \(args[2]): \(sizes.map(String.init).joined(separator: ", ")) px")
