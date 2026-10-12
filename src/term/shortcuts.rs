//! The Keyboard Shortcuts panel (⌘/): every menu command with its keys, read
//! from the menus each time so it can't go stale, plus the keys that aren't
//! menu commands.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSEventModifierFlags, NSFont, NSFontAttributeName,
    NSFontWeightSemibold, NSForegroundColorAttributeName, NSMenu, NSMutableParagraphStyle, NSPanel,
    NSParagraphStyleAttributeName, NSScrollView, NSTextAlignment, NSTextTab, NSTextView, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSMutableAttributedString, NSPoint, NSRect, NSSize, NSString,
};

/// The panel's size when the screen has room: large enough to show every
/// command at once, in columns.
const WIDTH: f64 = 860.0;
const HEIGHT: f64 = 580.0;
/// Margin around the columns, and the space between them.
const PAD: f64 = 22.0;
const GAP: f64 = 28.0;
/// Narrowest a column gets before there are fewer of them.
const MIN_COLUMN: f64 = 230.0;

/// Keys that aren't menu commands: (what, keys).
const TYPING: &[(&str, &str)] = &[
    ("Newline without sending (Claude Code)", "⇧↩"),
    ("Start / end of line", "⌘← / ⌘→"),
    ("Delete to start of line", "⌘⌫"),
    ("Word left / right", "⌥← / ⌥→"),
    ("Delete word", "⌥⌫"),
    ("Clear the screen (shell)", "⌃L"),
    ("Scroll back / forward", "⇧PgUp / ⇧PgDn"),
    ("Open a link or file path", "⌘-click"),
    ("Select a word / line", "double / triple click"),
];

thread_local! {
    static PANEL: RefCell<Option<Retained<NSPanel>>> = const { RefCell::new(None) };
}

/// Show the panel centred over the key window, or close it if it's showing.
/// Closed panels are dropped, so it holds no memory (window buffer, text)
/// while hidden.
pub fn toggle(mtm: MainThreadMarker) {
    if let Some(old) = PANEL.with(|p| p.borrow_mut().take())
        && old.isVisible()
    {
        old.close();
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    let window = app.keyWindow().or_else(|| app.mainWindow());
    // As large as WIDTH × HEIGHT, smaller on a small screen.
    let screen = window
        .as_ref()
        .and_then(|w| w.screen())
        .or_else(|| objc2_app_kit::NSScreen::mainScreen(mtm))
        .map(|s| s.visibleFrame());
    let size = NSSize::new(
        screen.map_or(WIDTH, |s| WIDTH.min(s.size.width - 80.0)),
        screen.map_or(HEIGHT, |s| HEIGHT.min(s.size.height - 120.0)),
    );
    let panel = build(mtm, size);
    PANEL.with(|p| *p.borrow_mut() = Some(panel.clone()));
    match window {
        Some(w) => {
            let f = w.frame();
            let p = panel.frame();
            panel.setFrameOrigin(NSPoint::new(
                f.origin.x + (f.size.width - p.size.width) / 2.0,
                f.origin.y + (f.size.height - p.size.height) / 2.0,
            ));
        }
        None => panel.center(),
    }
    panel.orderFront(None);
}

#[cfg(feature = "selftest")]
pub fn is_visible() -> bool {
    PANEL.with(|p| p.borrow().as_ref().is_some_and(|p| p.isVisible()))
}

/// Selftest: the panel's content, to snapshot it.
#[cfg(feature = "selftest")]
pub fn content_view() -> Option<Retained<objc2_app_kit::NSView>> {
    // The frame view, so the panel's background is in the picture too.
    PANEL.with(|p| {
        p.borrow().as_ref().and_then(|p| {
            p.displayIfNeeded();
            p.contentView().and_then(|c| unsafe { c.superview() })
        })
    })
}

/// The panel, `size` large, with the sections laid out in as many columns as
/// fit (three at full size), scrolling only if a column is still too tall.
fn build(mtm: MainThreadMarker, size: NSSize) -> Retained<NSPanel> {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        frame,
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::UtilityWindow,
        NSBackingStoreType::Buffered,
        false,
    );
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setTitle(&NSString::from_str("Keyboard Shortcuts"));
    panel.setFloatingPanel(true);
    panel.setHidesOnDeactivate(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    // 8-bit buffers, like the main window (half the memory of the default).
    panel.setColorSpace(Some(&objc2_app_kit::NSColorSpace::sRGBColorSpace()));

    let sections = sections(mtm);
    let n = column_count(size.width);
    let col_w = ((size.width - 2.0 * PAD - (n - 1) as f64 * GAP) / n as f64).floor();
    let weights: Vec<usize> = sections.iter().map(|(_, rows)| rows.len() + 2).collect();

    // Each column is a text view, measured, then placed top-aligned.
    let mut columns = Vec::new();
    for (c, range) in split_columns(&weights, n).into_iter().enumerate() {
        let out = NSMutableAttributedString::new();
        for (i, (title, rows)) in sections[range].iter().enumerate() {
            section(&out, title, rows, i == 0, col_w);
        }
        let text = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(col_w, 10.0)),
        );
        text.setEditable(false);
        text.setSelectable(false);
        text.setDrawsBackground(false);
        text.setTextContainerInset(NSSize::new(0.0, 0.0));
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        if let Some(storage) = unsafe { text.textStorage() } {
            storage.setAttributedString(&out);
        }
        text.sizeToFit();
        let x = PAD + c as f64 * (col_w + GAP);
        columns.push((text, x));
    }
    let tallest = columns
        .iter()
        .map(|(t, _)| t.frame().size.height)
        .fold(0.0, f64::max);
    let doc_h = (tallest + 2.0 * PAD).max(size.height);
    let doc = objc2_app_kit::NSView::initWithFrame(
        objc2_app_kit::NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(size.width, doc_h)),
    );
    for (text, x) in &columns {
        let h = text.frame().size.height;
        text.setFrame(NSRect::new(
            NSPoint::new(*x, doc_h - PAD - h),
            NSSize::new(col_w, h),
        ));
        doc.addSubview(text);
    }

    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    scroll.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    scroll.setDocumentView(Some(&doc));
    // Start at the top of the document (its origin is at the bottom).
    doc.scrollPoint(NSPoint::new(0.0, doc_h));
    if let Some(content) = panel.contentView() {
        content.addSubview(&scroll);
    }
    panel
}

/// Columns that fit `width`: three at full size, fewer on a small screen.
fn column_count(width: f64) -> usize {
    let mut n = 3;
    while n > 1 && (width - 2.0 * PAD - (n - 1) as f64 * GAP) / (n as f64) < MIN_COLUMN {
        n -= 1;
    }
    n
}

/// Split sections (by their line counts) into `n` runs, in order, as even
/// as possible: each column takes sections until it reaches its share.
fn split_columns(weights: &[usize], n: usize) -> Vec<std::ops::Range<usize>> {
    let total: usize = weights.iter().sum();
    let mut out = Vec::new();
    let mut start = 0;
    let mut done = 0;
    for col in 0..n {
        if start >= weights.len() {
            break;
        }
        if col == n - 1 {
            out.push(start..weights.len());
            break;
        }
        // This column's fair share of what's left.
        let target = (total - done).div_ceil(n - col);
        let mut end = start;
        let mut sum = 0;
        while end < weights.len() {
            let next = sum + weights[end];
            // Stop once the share is reached, or before a section that would
            // overshoot it by more than it fills; always take at least one.
            if end > start && (sum >= target || (next > target && next - target > target - sum)) {
                break;
            }
            sum = next;
            end += 1;
        }
        out.push(start..end);
        done += sum;
        start = end;
    }
    out
}

/// Sections: one per top-level menu with key equivalents, then typing keys.
fn sections(mtm: MainThreadMarker) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    let app = NSApplication::sharedApplication(mtm);
    if let Some(bar) = app.mainMenu() {
        for top in bar.itemArray().iter() {
            let Some(menu) = top.submenu() else { continue };
            let rows = rows_of(&menu);
            if rows.is_empty() {
                continue;
            }
            let title = if out.is_empty() {
                "Odek".to_string()
            } else {
                top.title().to_string()
            };
            out.push((title, rows));
        }
    }
    let typing: Vec<(String, String)> = TYPING
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    out.push(("Typing in a terminal".to_string(), typing));
    out
}

fn rows_of(menu: &NSMenu) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for item in menu.itemArray().iter() {
        if item.isSeparatorItem() || item.isHidden() {
            continue;
        }
        if let Some(sub) = item.submenu() {
            rows.extend(rows_of(&sub));
            continue;
        }
        let key = item.keyEquivalent().to_string();
        if key.is_empty() {
            continue;
        }
        let keys = format_keys(&key, item.keyEquivalentModifierMask());
        // ⌘/ means one thing in a terminal and another in the code viewer.
        let title = if item.action() == Some(objc2::sel!(appToggleComment:)) {
            "This panel (terminal) · Toggle Line Comment (code)".to_string()
        } else {
            item.title().to_string()
        };
        rows.push((title, keys));
    }
    rows
}

/// "⌃⌥⇧⌘K" in Apple's order, with names for the special keys.
pub fn format_keys(key: &str, mods: NSEventModifierFlags) -> String {
    let mut s = String::new();
    for (flag, sym) in [
        (NSEventModifierFlags::Control, "⌃"),
        (NSEventModifierFlags::Option, "⌥"),
        (NSEventModifierFlags::Shift, "⇧"),
        (NSEventModifierFlags::Command, "⌘"),
    ] {
        if mods.contains(flag) {
            s.push_str(sym);
        }
    }
    let name = match key.chars().next().map(|c| c as u32) {
        Some(0x09) => "⇥".to_string(),
        Some(0x0d) => "↩".to_string(),
        Some(0x7f) => "⌫".to_string(),
        Some(0x20) => "Space".to_string(),
        Some(0xF700) => "↑".to_string(),
        Some(0xF701) => "↓".to_string(),
        Some(0xF702) => "←".to_string(),
        Some(0xF703) => "→".to_string(),
        Some(0xF729) => "Home".to_string(),
        Some(0xF72B) => "End".to_string(),
        Some(0xF72C) => "PgUp".to_string(),
        Some(0xF72D) => "PgDn".to_string(),
        _ => key.to_uppercase(),
    };
    s.push_str(&name);
    s
}

fn section(out: &NSMutableAttributedString, title: &str, rows: &[(String, String)], first: bool, width: f64) {
    let head_font = NSFont::systemFontOfSize_weight(12.0, unsafe { NSFontWeightSemibold });
    let para = NSMutableParagraphStyle::new();
    para.setParagraphSpacingBefore(if first { 0.0 } else { 14.0 });
    para.setParagraphSpacing(4.0);
    append(
        out,
        &format!("{}\n", title.to_uppercase()),
        &head_font,
        &NSColor::secondaryLabelColor(),
        &para,
    );

    let font = NSFont::systemFontOfSize(13.0);
    let keys_font =
        NSFont::monospacedSystemFontOfSize_weight(12.5, unsafe { objc2_app_kit::NSFontWeightRegular });
    let row_para = NSMutableParagraphStyle::new();
    let tab = unsafe {
        NSTextTab::initWithTextAlignment_location_options(
            NSTextTab::alloc(),
            NSTextAlignment::Right,
            width - 2.0,
            &NSDictionary::new(),
        )
    };
    row_para.setTabStops(Some(&NSArray::from_retained_slice(&[tab])));
    row_para.setParagraphSpacing(3.0);
    for (what, keys) in rows {
        append(out, what, &font, &NSColor::labelColor(), &row_para);
        append(
            out,
            &format!("\t{keys}\n"),
            &keys_font,
            &NSColor::secondaryLabelColor(),
            &row_para,
        );
    }
}

fn append(
    out: &NSMutableAttributedString,
    text: &str,
    font: &NSFont,
    color: &NSColor,
    para: &NSMutableParagraphStyle,
) {
    let attrs: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
        NSDictionary::from_slices(
            &[
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
            ],
            &[font as &AnyObject, color as &AnyObject, para as &AnyObject],
        )
    };
    let piece = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attrs),
        )
    };
    out.appendAttributedString(&piece);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fit_the_width() {
        assert_eq!(column_count(WIDTH), 3);
        assert_eq!(column_count(600.0), 2);
        assert_eq!(column_count(420.0), 1);
    }

    #[test]
    fn sections_split_evenly_in_order() {
        // Every section once, in order, in at most n columns.
        for (weights, n) in [
            (vec![7, 11, 4, 6, 24, 11], 3),
            (vec![5, 5, 5, 5, 5, 5], 3),
            (vec![40, 2, 2], 3),
            (vec![3], 3),
            (vec![4, 4], 1),
        ] {
            let cols = split_columns(&weights, n);
            assert!(cols.len() <= n);
            let flat: Vec<usize> = cols.iter().flat_map(|r| r.clone()).collect();
            assert_eq!(flat, (0..weights.len()).collect::<Vec<_>>(), "{weights:?}");
            assert!(cols.iter().all(|r| !r.is_empty()));
        }
        // Even weights share out evenly.
        assert_eq!(split_columns(&[5, 5, 5, 5, 5, 5], 3), [0..2, 2..4, 4..6]);
        // A long section stays whole in its column; the rest share the others.
        assert_eq!(split_columns(&[40, 2, 2], 3), [0..1, 1..2, 2..3]);
        // The menus as they are today: Odek, Shell, File, Edit, View, Window,
        // typing keys.
        let cols = split_columns(&[8, 15, 7, 21, 9, 8, 11], 3);
        let sums: Vec<usize> = cols
            .iter()
            .map(|r| [8, 15, 7, 21, 9, 8, 11][r.clone()].iter().sum())
            .collect();
        assert!(sums.iter().max().unwrap() - sums.iter().min().unwrap() <= 12, "{cols:?} {sums:?}");
    }

    #[test]
    fn key_names() {
        let cmd = NSEventModifierFlags::Command;
        assert_eq!(format_keys("l", cmd), "⌘L");
        assert_eq!(format_keys("l", cmd | NSEventModifierFlags::Control), "⌃⌘L");
        assert_eq!(format_keys("\u{F700}", cmd), "⌘↑");
        assert_eq!(
            format_keys("\u{F72C}", cmd | NSEventModifierFlags::Option),
            "⌥⌘PgUp"
        );
        assert_eq!(
            format_keys("\t", NSEventModifierFlags::Control | NSEventModifierFlags::Shift),
            "⌃⇧⇥"
        );
    }
}
