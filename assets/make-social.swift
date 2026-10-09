// Renders docs/social-preview.png (1280 × 640, GitHub's link-preview size):
// the wordmark lockup and a line of copy on the ink background.
// swift assets/make-social.swift   (run from the repository root)
import AppKit

let W = 1280, H = 640
func hex(_ h: UInt32) -> NSColor {
    NSColor(srgbRed: CGFloat((h >> 16) & 255) / 255, green: CGFloat((h >> 8) & 255) / 255,
            blue: CGFloat(h & 255) / 255, alpha: 1)
}

// Draw into an exact-size bitmap so Retina screens don't double it.
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: W, pixelsHigh: H, bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                           colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

hex(0x0B0F14).setFill()
NSRect(x: 0, y: 0, width: W, height: H).fill()

let lockup = NSImage(contentsOfFile: "assets/odek-lockup.png")!
let lw: CGFloat = 840, lh = lw * 560 / 1960
lockup.draw(in: NSRect(x: (CGFloat(W) - lw) / 2, y: 300, width: lw, height: lh))

let copy = "A tiny, native terminal for macOS,\nmade for running coding agents side by side."
let para = NSMutableParagraphStyle()
para.alignment = .center
para.lineSpacing = 8
let attrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.monospacedSystemFont(ofSize: 30, weight: .regular),
    .foregroundColor: hex(0x8B98A5),
    .paragraphStyle: para,
]
(copy as NSString).draw(in: NSRect(x: 80, y: 130, width: CGFloat(W) - 160, height: 110), withAttributes: attrs)

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "docs/social-preview.png"))
print("wrote docs/social-preview.png")
