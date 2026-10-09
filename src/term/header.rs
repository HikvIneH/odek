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
            NSColor::separatorColor().setFill();
            NSBezierPath::fillRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(b.size.width, 1.0)));

            let color = if self.ivars().focused.get() {
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
