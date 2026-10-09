//! Connects the Vim engine (`crate::vim`) to the editor: key events in,
//! NSTextView edits out, plus the mode label and block cursor.

use std::ptr;

use objc2::rc::Retained;
use objc2::{DefinedClass, msg_send};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventModifierFlags, NSFontAttributeName,
    NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSStringDrawing, NSTextInputClient, NSTextView,
};
use objc2_foundation::{NSPoint, NSRange, NSString, NSStringCompareOptions, NSUndoManager, NSUserDefaults};

use super::{App, attrs, selected_range};
use crate::highlight::Lang;
use crate::vim::{Buffer, Effect, Key, Scroll};

/// NSTextView as a Vim buffer. Positions are UTF-16 offsets, like NSString.
struct TextViewBuffer<'a> {
    app: &'a App,
    tv: &'a NSTextView,
    text: Retained<NSString>,
    lang: Option<Lang>,
}

impl TextViewBuffer<'_> {
    fn reveal(&self, at: usize) {
        self.tv.scrollRangeToVisible(NSRange::new(at, 0));
    }
}

impl Buffer for TextViewBuffer<'_> {
    fn len(&self) -> usize {
        self.text.length()
    }

    fn at(&self, i: usize) -> u16 {
        self.text.characterAtIndex(i)
    }

    fn slice(&self, start: usize, end: usize) -> String {
        self.text
            .substringWithRange(NSRange::new(start, end.saturating_sub(start)))
            .to_string()
    }

    fn cursor(&self) -> usize {
        selected_range(self.tv).location
    }

    fn set_cursor(&mut self, i: usize) {
        let i = i.min(self.len());
        self.tv.setSelectedRange(NSRange::new(i, 0));
        self.reveal(i);
    }

    fn set_selection(&mut self, start: usize, end: usize) {
        self.tv
            .setSelectedRange(NSRange::new(start, end.saturating_sub(start)));
        self.reveal(end.saturating_sub(1).max(start));
    }

    fn replace(&mut self, start: usize, end: usize, text: &str) {
        // insertText registers undo and fires textDidChange (dirty, highlight).
        unsafe {
            self.tv
                .insertText_replacementRange(&NSString::from_str(text), NSRange::new(start, end - start))
        };
        self.text = self.tv.string();
    }

    fn undo(&mut self) {
        let um: Option<Retained<NSUndoManager>> = unsafe { msg_send![self.tv, undoManager] };
        if let Some(um) = um.filter(|u| u.canUndo()) {
            um.undo();
        }
        self.text = self.tv.string();
    }

    fn redo(&mut self) {
        let um: Option<Retained<NSUndoManager>> = unsafe { msg_send![self.tv, undoManager] };
        if let Some(um) = um.filter(|u| u.canRedo()) {
            um.redo();
        }
        self.text = self.tv.string();
    }

    fn page_lines(&self) -> usize {
        let font = self.app.font();
        let a = attrs(&[(unsafe { NSFontAttributeName }, &*font)]);
        let h =
            unsafe { NSString::from_str("M").sizeWithAttributes(Some(&a)) }.height + self.app.line_spacing();
        (self.tv.visibleRect().size.height / h.max(1.0)) as usize
    }

    fn scroll(&mut self, pos: usize, how: Scroll) {
        let (Some(lm), visible) = (unsafe { self.tv.layoutManager() }, self.tv.visibleRect()) else {
            return;
        };
        let glyph = lm.glyphIndexForCharacterAtIndex(pos.min(self.len()));
        let frag = unsafe { lm.lineFragmentRectForGlyphAtIndex_effectiveRange(glyph, ptr::null_mut()) };
        let y = frag.origin.y + self.tv.textContainerOrigin().y;
        let top = match how {
            Scroll::Top => y,
            Scroll::Center => y - (visible.size.height - frag.size.height) / 2.0,
            Scroll::Bottom => y - visible.size.height + frag.size.height,
        };
        self.tv.scrollPoint(NSPoint::new(visible.origin.x, top.max(0.0)));
    }

    fn find(&self, needle: &str, from: usize, forward: bool) -> Option<usize> {
        let n = self.len();
        let pat = NSString::from_str(needle);
        let search = |range: NSRange, back: bool| {
            let opts = if back {
                NSStringCompareOptions::BackwardsSearch
            } else {
                NSStringCompareOptions::empty()
            };
            let r = self.text.rangeOfString_options_range(&pat, opts, range);
            (r.length > 0).then_some(r.location)
        };
        if forward {
            let start = (from + 1).min(n);
            search(NSRange::new(start, n - start), false).or_else(|| search(NSRange::new(0, n), false))
        } else {
            search(NSRange::new(0, from.min(n)), true).or_else(|| search(NSRange::new(0, n), true))
        }
    }

    fn line_offset(&self, line: usize) -> usize {
        let starts = self.app.ui().ruler.line_starts(self.tv);
        starts[line.min(starts.len() - 1)] as usize
    }

    fn clipboard(&self) -> Option<String> {
        NSPasteboard::generalPasteboard()
            .stringForType(unsafe { NSPasteboardTypeString })
            .map(|s| s.to_string())
    }

    fn set_clipboard(&mut self, text: &str) {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
    }

    /// Tabs for Go and for files indented with tabs; otherwise the file's
    /// smallest space indent (2 or 4), defaulting to 4.
    fn indent_unit(&self) -> String {
        if self.lang == Some(Lang::Go) {
            return "\t".into();
        }
        let sample = self.slice(0, self.len().min(20_000));
        let mut smallest = usize::MAX;
        for line in sample.lines() {
            if line.starts_with('\t') {
                return "\t".into();
            }
            let spaces = line.len() - line.trim_start_matches(' ').len();
            if spaces > 0 && spaces < line.len() {
                smallest = smallest.min(spaces);
            }
        }
        " ".repeat(if smallest == 2 { 2 } else { 4 })
    }
}

/// NSEvent → Vim key. `None` = not ours (⌘ shortcuts, arrows, function keys).
fn key_of(event: &NSEvent) -> Option<Key> {
    let flags = event.modifierFlags();
    if flags.contains(NSEventModifierFlags::Command) {
        return None;
    }
    match event.keyCode() {
        53 => return Some(Key::Esc),
        36 | 76 => return Some(Key::Enter),
        51 => return Some(Key::Backspace),
        _ => {}
    }
    let plain = event.charactersIgnoringModifiers()?.to_string();
    let c = plain.chars().next()?;
    if ('\u{F700}'..='\u{F8FF}').contains(&c) {
        return None;
    }
    if flags.contains(NSEventModifierFlags::Control) {
        return Some(Key::Ctrl(c.to_ascii_lowercase()));
    }
    let typed = event.characters()?.to_string();
    typed.chars().next().filter(|c| !c.is_control()).map(Key::Char)
}

impl App {
    /// Feed a key to Vim. Returns false to let the text view handle it.
    pub(super) fn vim_key(&self, event: &NSEvent) -> bool {
        if !self.ivars().vim_on.get() {
            return false;
        }
        let (current, lang) = {
            let tabs = self.ivars().tabs.borrow();
            (tabs.current, tabs.current.and_then(|i| tabs.list[i].lang))
        };
        let Some(current) = current else { return false };
        let Some(key) = key_of(event) else { return false };
        let ui = self.ui();
        let effect = {
            let mut vim = self.ivars().vim.borrow_mut();
            let mut buf = TextViewBuffer {
                app: self,
                tv: &ui.text,
                text: ui.text.string(),
                lang,
            };
            vim.handle(key, &mut buf)
        };
        let handled = effect != Effect::PassThrough;
        match effect {
            Effect::Save => {
                self.save_tab(current);
            }
            Effect::SaveClose => {
                if self.save_tab(current) {
                    self.remove_tab(current);
                }
            }
            Effect::Close { force } => {
                let dirty = self
                    .ivars()
                    .tabs
                    .borrow()
                    .list
                    .get(current)
                    .is_some_and(|t| t.dirty);
                if dirty && !force {
                    self.ivars().vim.borrow_mut().message =
                        Some("E37: No write since last change (add ! to override)".into());
                } else {
                    self.remove_tab(current);
                }
            }
            Effect::NextTab => self.cycle_tab(1),
            Effect::PrevTab => self.cycle_tab(-1),
            Effect::None | Effect::PassThrough => {}
        }
        self.vim_update_ui();
        handled
    }

    /// Mode label + block cursor.
    pub(super) fn vim_update_ui(&self) {
        let ui = self.ui();
        let on = self.ivars().vim_on.get();
        let has_tab = self.ivars().tabs.borrow().current.is_some();
        let vim = self.ivars().vim.borrow();
        ui.vim_label.setHidden(!on || !has_tab);
        ui.vim_label.setStringValue(&NSString::from_str(&vim.status()));
        let font = self.font();
        let a = attrs(&[(unsafe { NSFontAttributeName }, &*font)]);
        let w = unsafe { NSString::from_str("M").sizeWithAttributes(Some(&a)) }.width;
        ui.code.set_block(on && has_tab && vim.block_cursor(), w);
    }

    /// New tab or Vim switched on: start in normal mode.
    pub(super) fn vim_reset(&self) {
        self.ivars().vim.borrow_mut().reset();
        self.vim_update_ui();
    }

    pub(super) fn vim_toggle(&self, sender: Option<&NSMenuItem>) {
        let on = !self.ivars().vim_on.get();
        self.ivars().vim_on.set(on);
        NSUserDefaults::standardUserDefaults().setBool_forKey(on, objc2_foundation::ns_string!("vimMode"));
        if let Some(item) = sender {
            item.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        self.vim_reset();
    }
}
