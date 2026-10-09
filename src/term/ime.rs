//! Text-input-method state for `TermView`: marked (in-progress) text, the key
//! event being interpreted, and drawing the marked text at the cursor.

use std::cell::{Cell, RefCell};

use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBezierPath, NSColor, NSEvent, NSEventModifierFlags, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSStringDrawing, NSUnderlineStyleAttributeName,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSNumber, NSPoint, NSRange, NSRect, NSSize, NSString};
use unicode_width::UnicodeWidthChar;

pub const NOT_FOUND: usize = isize::MAX as usize;

#[derive(Default)]
pub struct Ime {
    marked: RefCell<String>,
    selected: Cell<(usize, usize)>,
    /// The keyDown being interpreted, so doCommandBySelector: can fall back to it.
    event: RefCell<Option<Retained<NSEvent>>>,
}

impl Ime {
    pub fn marked(&self) -> Option<String> {
        let m = self.marked.borrow();
        (!m.is_empty()).then(|| m.clone())
    }

    pub fn has_marked(&self) -> bool {
        !self.marked.borrow().is_empty()
    }

    pub fn set(&self, text: String, selected: NSRange) {
        *self.marked.borrow_mut() = text;
        self.selected.set((selected.location, selected.length));
    }

    /// True if there was marked text.
    pub fn clear(&self) -> bool {
        self.selected.set((0, 0));
        !std::mem::take(&mut *self.marked.borrow_mut()).is_empty()
    }

    pub fn marked_range(&self) -> NSRange {
        match self.marked.borrow().encode_utf16().count() {
            0 => NSRange::new(NOT_FOUND, 0),
            n => NSRange::new(0, n),
        }
    }

    pub fn selected_range(&self) -> NSRange {
        let (loc, len) = self.selected.get();
        NSRange::new(loc, len)
    }

    /// UTF-16 offset of the selection within the marked text, as a cell column offset.
    pub fn caret_cols(&self) -> usize {
        let m = self.marked.borrow();
        let at = self.selected.get().0;
        let mut u = 0;
        let mut cols = 0;
        for ch in m.chars() {
            if u >= at {
                break;
            }
            u += ch.len_utf16();
            cols += ch.width().unwrap_or(0);
        }
        cols
    }

    pub fn begin(&self, event: &NSEvent) {
        *self.event.borrow_mut() = Some(event.retain());
    }

    pub fn take_event(&self) -> Option<Retained<NSEvent>> {
        self.event.borrow_mut().take()
    }
}

/// Whether a keyDown should go through the input system: plain typing (no
/// ctrl/option/cmd; Option stays Meta), dead keys (empty characters), or
/// anything at all while an input method is composing.
pub fn wants(chars: &str, flags: NSEventModifierFlags, composing: bool) -> bool {
    let held = NSEventModifierFlags::Control | NSEventModifierFlags::Option | NSEventModifierFlags::Command;
    if flags.intersects(held) {
        return false;
    }
    composing
        || chars.chars().next().is_none_or(|c| {
            let c = c as u32;
            c >= 0x20 && c != 0x7f && !(0xF700..=0xF8FF).contains(&c)
        })
}

/// The NSString or NSAttributedString argument of insertText: / setMarkedText:.
pub fn string_of(obj: &AnyObject) -> String {
    match obj.downcast_ref::<NSAttributedString>() {
        Some(a) => a.string().to_string(),
        None => obj.downcast_ref::<NSString>().map_or_else(String::new, |s| s.to_string()),
    }
}

pub fn cols_of(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Draw marked text over the cells at the cursor: theme background, default
/// foreground, underlined. Clipped to the line.
#[allow(clippy::too_many_arguments)]
pub fn draw_marked(
    text: &str,
    origin: NSPoint,
    cw: f64,
    ch: f64,
    max_cols: usize,
    fg: &NSColor,
    bg: &NSColor,
    font: &NSFont,
) {
    let mut cols = 0;
    let shown: String = text
        .chars()
        .take_while(|c| {
            cols += c.width().unwrap_or(0);
            cols <= max_cols
        })
        .collect();
    let cols = cols_of(&shown);
    bg.setFill();
    NSBezierPath::fillRect(NSRect::new(origin, NSSize::new(cols as f64 * cw, ch)));
    let one = NSNumber::new_i32(1);
    let attrs: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
        NSDictionary::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName, NSUnderlineStyleAttributeName],
            &[font as &AnyObject, fg as &AnyObject, &*one as &AnyObject],
        )
    };
    unsafe { NSString::from_str(&shown).drawAtPoint_withAttributes(origin, Some(&attrs)) };
}
