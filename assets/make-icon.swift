// Generates odek app icon (1024) — chevron prompt + cursor on dark tile.
// swift make-icon.swift && iconutil -c icns odek.iconset
import AppKit
let S: CGFloat = 1024
let img = NSImage(size: NSSize(width: S, height: S))
img.lockFocus()
let ctx = NSGraphicsContext.current!.cgContext
ctx.translateBy(x: 0, y: S); ctx.scaleBy(x: 1, y: -1) // top-left origin
func hex(_ h: UInt32) -> CGColor { CGColor(red: CGFloat((h>>16)&255)/255, green: CGFloat((h>>8)&255)/255, blue: CGFloat(h&255)/255, alpha: 1) }
let bg = hex(0x0B0F14), fg = hex(0xE6EDF3), accent = hex(0x3B82F6)
ctx.setFillColor(bg)
ctx.addPath(CGPath(roundedRect: CGRect(x: 0, y: 0, width: S, height: S), cornerWidth: 230, cornerHeight: 230, transform: nil)); ctx.fillPath()
let k: CGFloat = S/160
let bw = 40*k, bh = 13*k, cx = 44*k
func bar(cy: CGFloat, angle: CGFloat, color: CGColor) {
  ctx.saveGState(); ctx.translateBy(x: cx, y: cy); ctx.rotate(by: angle)
  ctx.setFillColor(color)
  ctx.addPath(CGPath(roundedRect: CGRect(x: 0, y: -bh/2, width: bw, height: bh), cornerWidth: bh/2, cornerHeight: bh/2, transform: nil)); ctx.fillPath()
  ctx.restoreGState()
}
bar(cy: 57.5*k, angle: .pi/4, color: fg)
bar(cy: 102.5*k, angle: -.pi/4, color: fg)
ctx.setFillColor(accent)
ctx.addPath(CGPath(roundedRect: CGRect(x: 88*k, y: 92*k, width: 30*k, height: 13*k), cornerWidth: 6.5*k, cornerHeight: 6.5*k, transform: nil)); ctx.fillPath()
img.unlockFocus()
let png = NSBitmapImageRep(data: img.tiffRepresentation!)!.representation(using: .png, properties: [:])!
try! png.write(to: URL(fileURLWithPath: "icon-1024.png"))
print("wrote icon-1024.png")
