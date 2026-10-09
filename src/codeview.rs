//! NSTextView subclass for the editor:
//! - opts out of responsive scrolling (which would pre-render the whole
//!   document, ≈30 MB for 2,500 lines at 2×);
//! - routes keys through Vim mode first when it's on;
//! - draws a block cursor in Vim's normal mode.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSEvent, NSResponder, NSText, NSTextContainer, NSTextView, NSView,
};
use objc2_foundation::{NSObject, NSRect};

pub struct Ivars {
    /// Draw the caret as a block (Vim normal mode).
    pub block: Cell<bool>,
    /// Width of one character, for the block.
    pub char_w: Cell<f64>,
}

define_class!(
    #[unsafe(super(NSTextView, NSText, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct CodeView;

    impl CodeView {
        #[unsafe(method(isCompatibleWithResponsiveScrolling))]
        fn compatible_with_responsive_scrolling() -> bool {
            false
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            let ok: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if ok {
                crate::app::editor_focused();
            }
            ok
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !crate::app::vim_key_down(event) {
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }

        #[unsafe(method(drawInsertionPointInRect:color:turnedOn:))]
        fn draw_insertion_point(&self, rect: NSRect, color: &NSColor, on: bool) {
            if !self.ivars().block.get() {
                return unsafe { msg_send![super(self), drawInsertionPointInRect: rect, color: color, turnedOn: on] };
            }
            let mut r = rect;
            r.size.width = self.ivars().char_w.get();
            if on {
                color.colorWithAlphaComponent(0.45).setFill();
                NSBezierPath::fillRect(r);
            } else {
                self.setNeedsDisplayInRect_avoidAdditionalLayout(r, false);
            }
        }

        // The caret's dirty rect is sized for a thin bar; widen it so the
        // block is fully erased when it blinks off or moves.
        #[unsafe(method(setNeedsDisplayInRect:avoidAdditionalLayout:))]
        fn set_needs_display_in_rect(&self, rect: NSRect, avoid: bool) {
            let mut r = rect;
            if self.ivars().block.get() {
                r.size.width += self.ivars().char_w.get();
            }
            unsafe { msg_send![super(self), setNeedsDisplayInRect: r, avoidAdditionalLayout: avoid] }
        }
    }
);

impl CodeView {
    pub fn new(frame: NSRect, container: &NSTextContainer, mtm: MainThreadMarker) -> Retained<CodeView> {
        let this = CodeView::alloc(mtm).set_ivars(Ivars {
            block: Cell::new(false),
            char_w: Cell::new(7.0),
        });
        unsafe { msg_send![super(this), initWithFrame: frame, textContainer: container] }
    }

    pub fn set_block(&self, block: bool, char_w: f64) {
        let changed = self.ivars().block.get() != block;
        self.ivars().block.set(block);
        self.ivars().char_w.set(char_w);
        if changed {
            self.setNeedsDisplay(true);
            self.updateInsertionPointStateAndRestartTimer(true);
        }
    }
}

// Keep the `Retained<NSTextView>` view of it handy for code that only needs
// the text-view API.
pub fn as_text_view(v: &Retained<CodeView>) -> Retained<NSTextView> {
    Retained::into_super(v.clone())
}
