//! The strip above each pane of a split tab: its title, centred, brighter
//! for the focused pane. The close button is a subview added by the owner.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSLineBreakMode,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSResponder, NSStringDrawing, NSTextAlignment,
    NSView,
};
use objc2_foundation::{NSDictionary, NSObject, NSPoint, NSRect, NSSize, NSString};

pub struct Ivars {
    title: RefCell<String>,
    focused: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct PaneHeader;

    impl PaneHeader {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _rect: NSRect) {
            let b = self.bounds();
            NSColor::windowBackgroundColor().setFill();
            NSBezierPath::fillRect(b);
            let focused = self.ivars().focused.get();
            // The focused pane gets a green rule along the bottom and a green
            // dot before its title: a brighter title alone is hard to tell
            // apart in the light theme.
            let (rule, rule_h) = if focused {
                (NSColor::systemGreenColor(), 2.0)
            } else {
                (NSColor::separatorColor(), 1.0)
            };
            rule.setFill();
            NSBezierPath::fillRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(b.size.width, rule_h)));

            let color = if focused {
                NSColor::labelColor()
            } else {
                NSColor::tertiaryLabelColor()
            };
            let font = NSFont::systemFontOfSize(11.5);
            let para = NSMutableParagraphStyle::new();
            para.setAlignment(NSTextAlignment::Center);
            para.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            let attrs: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
                NSDictionary::from_slices(
                    &[NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName],
                    &[&*font as &AnyObject, &*color as &AnyObject, &*para as &AnyObject],
                )
            };
            // Leave room for the close button on the right, symmetric on the left.
            let rect = NSRect::new(NSPoint::new(30.0, 4.0), NSSize::new((b.size.width - 60.0).max(0.0), 16.0));
            let title = NSString::from_str(&self.ivars().title.borrow());
            unsafe { title.drawInRect_withAttributes(rect, Some(&attrs)) };
            if focused {
                let w = unsafe { title.sizeWithAttributes(Some(&attrs)) }.width.min(rect.size.width);
                let d = 7.0;
                let x = (rect.origin.x + (rect.size.width - w) / 2.0 - d - 6.0).max(6.0);
                let dot = NSRect::new(NSPoint::new(x, (b.size.height - d) / 2.0 + 1.0), NSSize::new(d, d));
                NSColor::systemGreenColor().setFill();
                NSBezierPath::bezierPathWithOvalInRect(dot).fill();
            }
        }
    }
);

impl PaneHeader {
    pub fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            title: RefCell::new(String::new()),
            focused: Cell::new(false),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn set(&self, title: &str, focused: bool) {
        if *self.ivars().title.borrow() != title || self.ivars().focused.get() != focused {
            *self.ivars().title.borrow_mut() = title.to_string();
            self.ivars().focused.set(focused);
            self.setNeedsDisplay(true);
        }
    }
}

define_class!(
    /// A pane: the header strip on top (when shown) and the pane's content
    /// filling the rest. It places both itself on every layout pass, so a
    /// window layout pass (the code viewer brings Auto Layout into the
    /// window) can't shrink the content to nothing.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct PaneBox;

    impl PaneBox {
        #[unsafe(method(layout))]
        fn layout(&self) {
            let _: () = unsafe { msg_send![super(self), layout] };
            self.place();
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.place();
        }
    }
);

impl PaneBox {
    pub fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Header at the top if visible, everything else below it.
    pub fn place(&self) {
        let b = self.bounds().size;
        let header = self
            .subviews()
            .iter()
            .find(|v| v.downcast_ref::<PaneHeader>().is_some() && !v.isHidden());
        let top = if header.is_some() { HEADER_H } else { 0.0 };
        for v in self.subviews().iter() {
            let frame = if v.downcast_ref::<PaneHeader>().is_some() {
                NSRect::new(
                    NSPoint::new(0.0, b.height - HEADER_H),
                    NSSize::new(b.width, HEADER_H),
                )
            } else {
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(b.width, (b.height - top).max(0.0)),
                )
            };
            if v.frame() != frame {
                v.setFrame(frame);
            }
        }
    }
}

/// Height of a pane's header strip.
pub const HEADER_H: f64 = 24.0;
