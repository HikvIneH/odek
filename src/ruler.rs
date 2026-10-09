//! Line-number gutter: an NSRulerView that draws only the visible numbers.

use std::cell::{Cell, RefCell};
use std::ptr;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSRulerOrientation,
    NSRulerView, NSScrollView, NSStringDrawing, NSTextView, NSView,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSString};

pub struct Ivars {
    /// UTF-16 offset where each line starts; rebuilt lazily after edits.
    starts: RefCell<Vec<u32>>,
    dirty: Cell<bool>,
    font: RefCell<Retained<NSFont>>,
    /// Extra space the paragraph style adds under each line.
    spacing: Cell<f64>,
}

define_class!(
    #[unsafe(super(NSRulerView, NSView, objc2_app_kit::NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct LineRuler;

    impl LineRuler {
        #[unsafe(method(drawHashMarksAndLabelsInRect:))]
        fn draw_hash_marks(&self, _rect: NSRect) {
            self.draw_numbers();
        }
    }
);

impl LineRuler {
    pub fn new(scroll: &NSScrollView, font_size: f64, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            starts: RefCell::new(vec![0]),
            dirty: Cell::new(true),
            font: RefCell::new(number_font(font_size)),
            spacing: Cell::new(0.0),
        });
        unsafe {
            msg_send![super(this), initWithScrollView: scroll, orientation: NSRulerOrientation::VerticalRuler]
        }
    }

    pub fn text_changed(&self) {
        self.ivars().dirty.set(true);
        self.setNeedsDisplay(true);
    }

    pub fn set_metrics(&self, font_size: f64, spacing: f64) {
        *self.ivars().font.borrow_mut() = number_font(font_size);
        self.ivars().spacing.set(spacing);
        self.text_changed();
    }

    /// Line starts of the current text, recomputed if the text changed.
    pub fn line_starts(&self, tv: &NSTextView) -> std::cell::Ref<'_, Vec<u32>> {
        if self.ivars().dirty.replace(false) {
            let text = tv.string().to_string();
            let mut starts = vec![0u32];
            for (i, unit) in text.encode_utf16().enumerate() {
                if unit == b'\n' as u16 {
                    starts.push(i as u32 + 1);
                }
            }
            *self.ivars().starts.borrow_mut() = starts;
        }
        self.ivars().starts.borrow()
    }

    fn draw_numbers(&self) {
        let bounds = self.bounds();
        NSColor::textBackgroundColor().setFill();
        NSBezierPath::fillRect(bounds);

        let Some(client) = self.clientView() else { return };
        let Some(tv) = client.downcast_ref::<NSTextView>() else {
            return;
        };
        let (Some(lm), Some(tc)) = (unsafe { tv.layoutManager() }, unsafe { tv.textContainer() }) else {
            return;
        };
        let len = tv.string().length();

        let starts = self.line_starts(tv);
        let font = self.ivars().font.borrow().clone();
        let color = NSColor::tertiaryLabelColor();
        let attrs: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
            NSDictionary::from_slices(
                &[NSFontAttributeName, NSForegroundColorAttributeName],
                &[&*font as &AnyObject, &*color as &AnyObject],
            )
        };

        // Widen the gutter when the line count gains a digit.
        let digits = starts.len().to_string().len().max(3);
        let digit_w = unsafe { NSString::from_str("8").sizeWithAttributes(Some(&attrs)) }.width;
        let thickness = (digits as f64 * digit_w + 22.0).ceil();
        if (self.ruleThickness() - thickness).abs() > 0.5 {
            self.setRuleThickness(thickness);
            return; // retile triggers another draw
        }

        let visible = tv.visibleRect();
        let glyphs = lm.glyphRangeForBoundingRect_inTextContainer(visible, &tc);
        let chars = unsafe { lm.characterRangeForGlyphRange_actualGlyphRange(glyphs, ptr::null_mut()) };
        let end = chars.location + chars.length;
        let first = starts
            .partition_point(|&s| s as usize <= chars.location)
            .saturating_sub(1);
        let origin_y = tv.textContainerOrigin().y;
        let spacing = self.ivars().spacing.get();
        let flipped = self.isFlipped();

        for (line, &start) in starts.iter().enumerate().skip(first) {
            let start = start as usize;
            if start > end {
                break;
            }
            let frag = if start >= len {
                lm.extraLineFragmentRect()
            } else {
                let glyph = lm.glyphIndexForCharacterAtIndex(start);
                unsafe { lm.lineFragmentRectForGlyphAtIndex_effectiveRange(glyph, ptr::null_mut()) }
            };
            if frag.size.height <= 0.0 {
                continue;
            }
            let label = NSString::from_str(&(line + 1).to_string());
            let size = unsafe { label.sizeWithAttributes(Some(&attrs)) };
            let text_h = frag.size.height - spacing;
            let top = self.convertPoint_fromView(NSPoint::new(0.0, frag.origin.y + origin_y), Some(tv));
            let pad = (text_h - size.height) / 2.0;
            let y = if flipped {
                top.y + pad
            } else {
                top.y - pad - size.height
            };
            let x = thickness - size.width - 10.0;
            unsafe { label.drawAtPoint_withAttributes(NSPoint::new(x, y), Some(&attrs)) };
        }
    }
}

fn number_font(size: f64) -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight((size - 1.0).max(8.0), unsafe {
        objc2_app_kit::NSFontWeightRegular
    })
}
