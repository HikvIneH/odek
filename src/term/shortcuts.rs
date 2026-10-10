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

const WIDTH: f64 = 420.0;
const HEIGHT: f64 = 620.0;

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

/// Show the panel beside the key window, or close it if it's showing. Closed
/// panels are dropped, so it holds no memory (window buffer, text) while hidden.
pub fn toggle(mtm: MainThreadMarker) {
    if let Some(old) = PANEL.with(|p| p.borrow_mut().take())
        && old.isVisible()
    {
        old.close();
        return;
    }
    let panel = build(mtm);
    PANEL.with(|p| *p.borrow_mut() = Some(panel.clone()));
    if let Some(text) = panel
        .contentView()
        .and_then(|c| c.subviews().firstObject())
        .and_then(|s| s.downcast::<NSScrollView>().ok())
        .and_then(|s| s.documentView())
        .and_then(|d| d.downcast::<NSTextView>().ok())
        && let Some(storage) = unsafe { text.textStorage() }
    {
        storage.setAttributedString(&contents(mtm));
    }
    let app = NSApplication::sharedApplication(mtm);
    if let Some(w) = app.keyWindow().or_else(|| app.mainWindow()) {
        let f = w.frame();
        panel.setFrameTopLeftPoint(NSPoint::new(
            f.origin.x + f.size.width - WIDTH - 16.0,
            f.origin.y + f.size.height - 48.0,
        ));
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

fn build(mtm: MainThreadMarker) -> Retained<NSPanel> {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        frame,
        NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::UtilityWindow,
        NSBackingStoreType::Buffered,
        false,
    );
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setTitle(&NSString::from_str("Keyboard Shortcuts"));
    panel.setFloatingPanel(true);
    panel.setHidesOnDeactivate(true);
    panel.setBecomesKeyOnlyIfNeeded(true);

    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    scroll.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
    text.setEditable(false);
    text.setSelectable(false);
    text.setDrawsBackground(false);
    text.setTextContainerInset(NSSize::new(14.0, 12.0));
    text.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
    scroll.setDocumentView(Some(&text));
    if let Some(content) = panel.contentView() {
        content.addSubview(&scroll);
    }
    panel
}

/// Sections: one per top-level menu with key equivalents, then typing keys.
fn contents(mtm: MainThreadMarker) -> Retained<NSAttributedString> {
    let out = NSMutableAttributedString::new();
    let app = NSApplication::sharedApplication(mtm);
    let mut first = true;
    if let Some(bar) = app.mainMenu() {
        for top in bar.itemArray().iter() {
            let Some(menu) = top.submenu() else { continue };
            let rows = rows_of(&menu);
            if rows.is_empty() {
                continue;
            }
            let title = if first {
                "Odek".to_string()
            } else {
                top.title().to_string()
            };
            section(&out, &title, &rows, first);
            first = false;
        }
    }
    let typing: Vec<(String, String)> = TYPING
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    section(&out, "Typing in a terminal", &typing, first);
    out.into_super()
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

fn section(out: &NSMutableAttributedString, title: &str, rows: &[(String, String)], first: bool) {
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
            WIDTH - 40.0,
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
