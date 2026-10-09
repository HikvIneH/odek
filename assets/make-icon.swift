// Draws the Odek app icon ("stanza": lines of code set like verse).
// Usage: swift assets/make-icon.swift <out.png> [pixels] [full|small32|small16]
// Small variants drop detail so the mark still reads at 16 and 32 px.
import AppKit

let args = CommandLine.arguments
let out = args.count > 1 ? args[1] : "icon-1024.png"
let px = args.count > 2 ? Int(args[2])! : 1024
let variant = args.count > 3 ? args[3] : "full"

let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px,
                           bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                           colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: a)
}

// Artwork is designed on a 100-unit grid where the tile spans 4…96, y down.
// That tile maps to the macOS icon grid: an 824 px tile inset 100 px in 1024.
let k = CGFloat(px) / 1024
func box(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> NSRect {
    let s = 824.0 / 92.0 * k
    return NSRect(x: (100 * k) + (x - 4) * s, y: CGFloat(px) - ((100 * k) + (y - 4 + h) * s), width: w * s, height: h * s)
}

let ink = rgb(0x241812), crema = rgb(0xC9792F)
rgb(0xF1E7DA).setFill()
NSBezierPath(roundedRect: box(4, 4, 92, 92), xRadius: 185 * k, yRadius: 185 * k).fill()

func bar(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ color: NSColor) {
    let r = box(x, y, w, h)
    color.setFill()
    NSBezierPath(roundedRect: r, xRadius: r.height / 2, yRadius: r.height / 2).fill()
}

switch variant {
case "small16":
    bar(20, 24, 56, 14, ink)
    bar(20, 58, 34, 14, crema)
case "small32":
    bar(20, 26, 52, 10, ink)
    bar(30, 45, 40, 10, ink.withAlphaComponent(0.55))
    bar(20, 64, 28, 10, crema)
default:
    bar(22, 27, 48, 7, ink)
    bar(30, 40, 36, 7, ink.withAlphaComponent(0.55))
    bar(30, 53, 44, 7, ink.withAlphaComponent(0.55))
    bar(22, 66, 24, 7, crema)
    crema.setFill()
    NSBezierPath(ovalIn: box(50.5, 66, 7, 7)).fill()
}

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
print("wrote \(out) (\(px) px, \(variant))")
