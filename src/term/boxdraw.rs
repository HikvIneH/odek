//! Box-drawing and block characters drawn as shapes instead of font glyphs,
//! so lines meet exactly at cell edges whatever the font and line height.
//! The caller sets the fill and stroke colour.

use objc2_app_kit::NSBezierPath;
use objc2_foundation::{NSPoint, NSRect, NSSize};

/// Line weights per arm: 0 none, 1 light, 2 heavy, 3 double.
#[derive(Clone, Copy)]
struct Arms {
    left: u8,
    right: u8,
    up: u8,
    down: u8,
}

const fn a(left: u8, right: u8, up: u8, down: u8) -> Option<Arms> {
    Some(Arms { left, right, up, down })
}

fn arms(c: u32) -> Option<Arms> {
    match c {
        0x2500 => a(1, 1, 0, 0),
        0x2501 => a(2, 2, 0, 0),
        0x2502 => a(0, 0, 1, 1),
        0x2503 => a(0, 0, 2, 2),
        0x250C => a(0, 1, 0, 1),
        0x250F => a(0, 2, 0, 2),
        0x2510 => a(1, 0, 0, 1),
        0x2513 => a(2, 0, 0, 2),
        0x2514 => a(0, 1, 1, 0),
        0x2517 => a(0, 2, 2, 0),
        0x2518 => a(1, 0, 1, 0),
        0x251B => a(2, 0, 2, 0),
        0x251C => a(0, 1, 1, 1),
        0x2523 => a(0, 2, 2, 2),
        0x2524 => a(1, 0, 1, 1),
        0x252B => a(2, 0, 2, 2),
        0x252C => a(1, 1, 0, 1),
        0x2533 => a(2, 2, 0, 2),
        0x2534 => a(1, 1, 1, 0),
        0x253B => a(2, 2, 2, 0),
        0x253C => a(1, 1, 1, 1),
        0x254B => a(2, 2, 2, 2),
        0x2550 => a(3, 3, 0, 0),
        0x2551 => a(0, 0, 3, 3),
        0x2554 => a(0, 3, 0, 3),
        0x2557 => a(3, 0, 0, 3),
        0x255A => a(0, 3, 3, 0),
        0x255D => a(3, 0, 3, 0),
        0x2560 => a(0, 3, 3, 3),
        0x2563 => a(3, 0, 3, 3),
        0x2566 => a(3, 3, 0, 3),
        0x2569 => a(3, 3, 3, 0),
        0x256C => a(3, 3, 3, 3),
        0x2574 => a(1, 0, 0, 0),
        0x2575 => a(0, 0, 1, 0),
        0x2576 => a(0, 1, 0, 0),
        0x2577 => a(0, 0, 0, 1),
        0x2578 => a(2, 0, 0, 0),
        0x2579 => a(0, 0, 2, 0),
        0x257A => a(0, 2, 0, 0),
        0x257B => a(0, 0, 0, 2),
        _ => None,
    }
}

/// Block elements as (x0, y0, x1, y1) fractions of the cell, top-left origin.
fn blocks(c: u32) -> Option<&'static [(f64, f64, f64, f64)]> {
    const TOP: (f64, f64, f64, f64) = (0.0, 0.0, 1.0, 0.5);
    const BOT: (f64, f64, f64, f64) = (0.0, 0.5, 1.0, 1.0);
    const TL: (f64, f64, f64, f64) = (0.0, 0.0, 0.5, 0.5);
    const TR: (f64, f64, f64, f64) = (0.5, 0.0, 1.0, 0.5);
    const BL: (f64, f64, f64, f64) = (0.0, 0.5, 0.5, 1.0);
    const BR: (f64, f64, f64, f64) = (0.5, 0.5, 1.0, 1.0);
    Some(match c {
        0x2580 => &[TOP],
        0x2584 => &[BOT],
        0x2588 => &[(0.0, 0.0, 1.0, 1.0)],
        0x258C => &[(0.0, 0.0, 0.5, 1.0)],
        0x2590 => &[(0.5, 0.0, 1.0, 1.0)],
        0x2594 => &[(0.0, 0.0, 1.0, 0.125)],
        0x2581 => &[(0.0, 0.875, 1.0, 1.0)],
        0x2596 => &[BL],
        0x2597 => &[BR],
        0x2598 => &[TL],
        0x259D => &[TR],
        0x2599 => &[TL, BL, BR],
        0x259A => &[TL, BR],
        0x259B => &[TL, TR, BL],
        0x259C => &[TL, TR, BR],
        0x259E => &[TR, BL],
        0x259F => &[TR, BL, BR],
        _ => return None,
    })
}

fn rounded(c: u32) -> bool {
    (0x256D..=0x2570).contains(&c)
}

pub fn is_drawn(c: u32) -> bool {
    arms(c).is_some() || blocks(c).is_some() || rounded(c)
}

/// Fill with edges snapped to device pixels, so neighbouring cells meet
/// without an anti-aliased seam.
fn fill(x: f64, y: f64, w: f64, h: f64, scale: f64) {
    let snap = |v: f64| (v * scale).round() / scale;
    let (x0, y0, x1, y1) = (snap(x), snap(y), snap(x + w), snap(y + h));
    NSBezierPath::fillRect(NSRect::new(NSPoint::new(x0, y0), NSSize::new(x1 - x0, y1 - y0)));
}

/// Draw `c` into `r` (a flipped view's cell rect); `scale` is the backing scale.
pub fn draw(c: u32, r: NSRect, scale: f64) {
    let fill = |x, y, w, h| fill(x, y, w, h, scale);
    let (x, y, w, h) = (r.origin.x, r.origin.y, r.size.width, r.size.height);
    if let Some(rects) = blocks(c) {
        for &(x0, y0, x1, y1) in rects {
            fill(x + x0 * w, y + y0 * h, (x1 - x0) * w, (y1 - y0) * h);
        }
        return;
    }
    let light = (w / 8.0).round().max(1.0);
    let heavy = light * 2.0;
    // Centre lines on whole points so they stay crisp.
    let cx = (x + w / 2.0 - light / 2.0).round();
    let cy = (y + h / 2.0 - light / 2.0).round();

    if rounded(c) {
        let path = NSBezierPath::bezierPath();
        path.setLineWidth(light);
        let (mx, my) = (cx + light / 2.0, cy + light / 2.0);
        let radius = (w / 2.0).min(h / 2.0);
        // ╭ ╮ ╯ ╰: which edges the curve runs to.
        let (hx, vy) = match c {
            0x256D => (x + w, y + h),
            0x256E => (x, y + h),
            0x256F => (x, y),
            _ => (x + w, y),
        };
        let sx = if hx > mx { 1.0 } else { -1.0 };
        let sy = if vy > my { 1.0 } else { -1.0 };
        path.moveToPoint(NSPoint::new(mx, vy));
        path.lineToPoint(NSPoint::new(mx, my + sy * radius));
        path.curveToPoint_controlPoint1_controlPoint2(
            NSPoint::new(mx + sx * radius, my),
            NSPoint::new(mx, my),
            NSPoint::new(mx, my),
        );
        path.lineToPoint(NSPoint::new(hx, my));
        path.stroke();
        return;
    }

    let Some(arm) = arms(c) else { return };
    let thick = |wt: u8| if wt == 2 { heavy } else { light };
    // Horizontal arms.
    for (wt, from, to) in [(arm.left, x, cx + light), (arm.right, cx, x + w)] {
        match wt {
            0 => {}
            3 => {
                fill(from, cy - light, to - from, light);
                fill(from, cy + light, to - from, light);
            }
            _ => {
                let t = thick(wt);
                fill(from, cy + light / 2.0 - t / 2.0, to - from, t);
            }
        }
    }
    // Vertical arms.
    for (wt, from, to) in [(arm.up, y, cy + light), (arm.down, cy, y + h)] {
        match wt {
            0 => {}
            3 => {
                fill(cx - light, from, light, to - from);
                fill(cx + light, from, light, to - from);
            }
            _ => {
                let t = thick(wt);
                fill(cx + light / 2.0 - t / 2.0, from, t, to - from);
            }
        }
    }
}
