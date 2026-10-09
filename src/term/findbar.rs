//! The Find bar: a small overlay at the top-right of a `TermView` with a
//! search field, an n/m label and prev/next buttons. Matches come from
//! `find::find` and are recomputed when the query changes or the user steps;
//! output that arrives while the bar is open is not searched, so highlights
//! can go stale until the next keystroke or step.

use objc2::rc::{Retained, Weak};
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBezelStyle, NSBezierPath, NSButton, NSColor, NSControl, NSControlSize,
    NSControlTextEditingDelegate, NSEventModifierFlags, NSFont, NSSearchField, NSSearchFieldDelegate, NSTextField, NSTextFieldDelegate,
    NSTextView, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{NSNotification, NSPoint, NSRect, NSSize, NSString};

use super::find::{self, Found};
use super::view::TermView;

const BAR_W: f64 = 300.0;
const BAR_H: f64 = 30.0;

pub struct FindState {
    found: Found,
    /// Current match number (meaningful when `found.count > 0`).
    cur: usize,
    field: Retained<NSSearchField>,
    label: Retained<NSTextField>,
    bar: Retained<NSView>,
    _target: Retained<Target>,
}

pub struct TargetIvars {
    view: Weak<TermView>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSControlTextEditingDelegate for Target {
        #[unsafe(method(controlTextDidChange:))]
        fn text_changed(&self, _n: &NSNotification) {
            if let Some(v) = self.ivars().view.load() {
                requery(&v);
            }
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _c: &NSControl, _tv: &NSTextView, cmd: Sel) -> Bool {
            let Some(v) = self.ivars().view.load() else { return Bool::NO };
            if cmd == sel!(insertNewline:) {
                let app = NSApplication::sharedApplication(self.mtm());
                let shift = app.currentEvent().is_some_and(|e| e.modifierFlags().contains(NSEventModifierFlags::Shift));
                step(&v, if shift { -1 } else { 1 });
                Bool::YES
            } else if cmd == sel!(cancelOperation:) {
                close(&v);
                Bool::YES
            } else {
                Bool::NO
            }
        }
    }

    unsafe impl NSTextFieldDelegate for Target {}

    unsafe impl NSSearchFieldDelegate for Target {}

    impl Target {
        #[unsafe(method(findPrev:))]
        fn prev(&self, _s: Option<&NSObject>) {
            if let Some(v) = self.ivars().view.load() {
                step(&v, -1);
            }
        }

        #[unsafe(method(findNext:))]
        fn next(&self, _s: Option<&NSObject>) {
            if let Some(v) = self.ivars().view.load() {
                step(&v, 1);
            }
        }
    }
);

/// Show the bar (or refocus it) and select its text.
pub(super) fn open(v: &TermView) {
    if let Some(st) = v.ivars().find.borrow().as_ref() {
        if let Some(w) = v.window() {
            w.makeFirstResponder(Some(&st.field));
        }
        unsafe { st.field.selectText(None) };
        return;
    }
    let mtm = MainThreadMarker::from(v);
    let Some(me) = (unsafe { Retained::retain(v as *const TermView as *mut TermView) }) else { return };
    let this = Target::alloc(mtm).set_ivars(TargetIvars { view: Weak::from_retained(&me) });
    let target: Retained<Target> = unsafe { msg_send![super(this), init] };

    let width = v.bounds().size.width;
    let frame = NSRect::new(NSPoint::new((width - BAR_W - 12.0).max(0.0), 8.0), NSSize::new(BAR_W, BAR_H));
    let bar = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
    bar.setMaterial(NSVisualEffectMaterial::HUDWindow);
    bar.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    bar.setState(NSVisualEffectState::Active);
    bar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin);

    let field = NSSearchField::initWithFrame(
        NSSearchField::alloc(mtm),
        NSRect::new(NSPoint::new(4.0, 4.0), NSSize::new(180.0, 22.0)),
    );
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*target))) };
    field.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    field.setPlaceholderString(Some(&NSString::from_str("Find")));

    let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    label.setFrame(NSRect::new(NSPoint::new(188.0, 7.0), NSSize::new(60.0, 16.0)));
    label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));

    let button = |title: &str, x: f64, action: Sel| {
        let b = unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str(title), Some(&target), Some(action), mtm)
        };
        b.setFrame(NSRect::new(NSPoint::new(x, 4.0), NSSize::new(24.0, 22.0)));
        b.setBezelStyle(NSBezelStyle::Toolbar);
        b.setControlSize(NSControlSize::Small);
        b
    };
    let up = button("\u{2191}", 250.0, sel!(findPrev:));
    let down = button("\u{2193}", 274.0, sel!(findNext:));
    bar.addSubview(&field);
    bar.addSubview(&label);
    bar.addSubview(&up);
    bar.addSubview(&down);
    v.addSubview(&bar);
    if let Some(w) = v.window() {
        w.makeFirstResponder(Some(&field));
    }
    *v.ivars().find.borrow_mut() = Some(FindState {
        found: Found::default(),
        cur: 0,
        field,
        label,
        bar: Retained::into_super(bar),
        _target: target,
    });
}

pub(super) fn close(v: &TermView) {
    let Some(st) = v.ivars().find.borrow_mut().take() else { return };
    st.bar.removeFromSuperview();
    if let Some(w) = v.window() {
        w.makeFirstResponder(Some(v));
    }
    v.setNeedsDisplay(true);
}

/// Open with `query` typed in and jump to the match (selftest).
#[cfg(feature = "selftest")]
pub(super) fn open_with(v: &TermView, query: &str) {
    open(v);
    if let Some(st) = v.ivars().find.borrow().as_ref() {
        st.field.setStringValue(&NSString::from_str(query));
    }
    requery(v);
}

fn query(v: &TermView) -> Option<String> {
    v.ivars().find.borrow().as_ref().map(|s| s.field.stringValue().to_string())
}

/// The query changed: search again and pick the match nearest the bottom of the view.
fn requery(v: &TermView) {
    let Some(q) = query(v) else { return };
    let Some((found, bottom)) = v.with_term(|t| (find::find(t, &q), t.first_id() + (v.top_index(t) + t.rows) as u64))
    else {
        return;
    };
    let cur = found.segs.iter().rev().find(|s| s.id < bottom).map_or(0, |s| s.m as usize);
    if let Some(st) = v.ivars().find.borrow_mut().as_mut() {
        st.found = found;
        st.cur = cur;
    }
    show_current(v);
}

/// Move to the next (1) or previous (-1) match, searching again first.
pub(super) fn step(v: &TermView, dir: isize) {
    let Some(q) = query(v) else { return };
    let Some(found) = v.with_term(|t| find::find(t, &q)) else { return };
    if let Some(st) = v.ivars().find.borrow_mut().as_mut() {
        let n = found.count as isize;
        if n > 0 {
            st.cur = (st.cur as isize + dir).rem_euclid(n) as usize;
        }
        st.found = found;
    }
    show_current(v);
}

fn show_current(v: &TermView) {
    let id = {
        let g = v.ivars().find.borrow();
        let Some(st) = g.as_ref() else { return };
        let n = st.found.count;
        let text = match (n, st.found.capped) {
            (0, _) if st.field.stringValue().length() == 0 => String::new(),
            (0, _) => "No results".into(),
            (_, true) => format!("{}/{n}+", st.cur + 1),
            _ => format!("{}/{n}", st.cur + 1),
        };
        st.label.setStringValue(&NSString::from_str(&text));
        st.found.of(st.cur).first().map(|s| s.id)
    };
    if let Some(id) = id {
        reveal(v, id);
    }
    v.setNeedsDisplay(true);
}

/// Scroll so line `id` is on screen (a third of the way down if it wasn't).
fn reveal(v: &TermView, id: u64) {
    let Some((idx, top, rows, alt)) =
        v.with_term(|t| (id.saturating_sub(t.first_id()) as isize, v.top_index(t) as isize, t.rows as isize, t.alt_active))
    else {
        return;
    };
    if alt || (top..top + rows).contains(&idx) {
        return;
    }
    v.scroll_lines(top - (idx - rows / 3).max(0));
}

/// Fill the highlighted cells of line `id`; `rect(start, end)` maps columns to a rect.
pub(super) fn paint(v: &TermView, id: u64, rect: impl Fn(usize, usize) -> NSRect) {
    let g = v.ivars().find.borrow();
    let Some(st) = g.as_ref() else { return };
    let lo = st.found.segs.partition_point(|s| s.id < id);
    for s in st.found.segs[lo..].iter().take_while(|s| s.id == id) {
        let c = if s.m as usize == st.cur {
            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.6, 0.1, 0.65)
        } else {
            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.85, 0.2, 0.30)
        };
        c.setFill();
        NSBezierPath::fillRect(rect(s.start, s.end));
    }
}
