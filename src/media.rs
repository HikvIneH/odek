//! Images and PDFs in the viewer: shown over the editor instead of the
//! "Binary file" line. Images go in an NSImageView (scaled down to fit,
//! never up); PDFs in PDFKit's PDFView, looked up at runtime so the only
//! cost is linking the system framework.

use std::path::Path;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBox, NSBoxType, NSColor, NSImage, NSImageAlignment, NSImageScaling,
    NSImageView, NSTitlePosition, NSView,
};
use objc2_foundation::{NSRect, NSSize, NSString, NSURL};

#[link(name = "PDFKit", kind = "framework")]
unsafe extern "C" {}

/// PDFView's kPDFDisplaySinglePageContinuous.
const PDF_SINGLE_PAGE_CONTINUOUS: isize = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Pdf,
}

/// What a file shows as, by extension. SVG stays text: it's source.
pub fn kind(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some(Kind::Pdf),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "avif" | "tif" | "tiff" | "bmp"
        | "ico" | "icns" => Some(Kind::Image),
        _ => None,
    }
}

pub struct Media {
    /// Sits over the editor's scroll view; hidden while a text file shows.
    pub root: Retained<NSBox>,
    image: Retained<NSImageView>,
    /// A PDFView, made on first use.
    pdf: std::cell::OnceCell<Retained<NSView>>,
}

impl Media {
    pub fn new(frame: NSRect, mtm: MainThreadMarker) -> Self {
        let root = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
        root.setBoxType(NSBoxType::Custom);
        root.setTitlePosition(NSTitlePosition::NoTitle);
        root.setBorderWidth(0.0);
        root.setFillColor(&NSColor::textBackgroundColor());
        root.setContentViewMargins(NSSize::new(0.0, 0.0));
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        root.setHidden(true);

        let bounds = root.contentView().map(|v| v.bounds()).unwrap_or(frame);
        let image = NSImageView::initWithFrame(NSImageView::alloc(mtm), bounds.inset(12.0));
        image.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        image.setImageAlignment(NSImageAlignment::AlignCenter);
        image.setAnimates(true);
        image.setEditable(false);
        image.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        image.setHidden(true);
        if let Some(c) = root.contentView() {
            c.addSubview(&image);
        }
        Self {
            root,
            image,
            pdf: std::cell::OnceCell::new(),
        }
    }

    /// Show `path`; false if it couldn't be read as that kind.
    pub fn show(&self, path: &Path, kind: Kind) -> bool {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let ok = match kind {
            Kind::Image => {
                let img = NSImage::initWithContentsOfURL(NSImage::alloc(), &url);
                self.image.setImage(img.as_deref());
                self.image.setHidden(img.is_none());
                if let Some(p) = self.pdf.get() {
                    p.setHidden(true);
                }
                img.is_some()
            }
            Kind::Pdf => {
                self.image.setImage(None);
                self.image.setHidden(true);
                match (self.pdf_view(), AnyClass::get(c"PDFDocument")) {
                    (Some(view), Some(cls)) => unsafe {
                        let doc: Allocated<AnyObject> = msg_send![cls, alloc];
                        let doc: Option<Retained<AnyObject>> = msg_send![doc, initWithURL: &*url];
                        let _: () = msg_send![&*view, setDocument: doc.as_deref()];
                        view.setHidden(doc.is_none());
                        doc.is_some()
                    },
                    _ => false,
                }
            }
        };
        self.root.setHidden(!ok);
        ok
    }

    pub fn hide(&self) {
        if self.root.isHidden() {
            return;
        }
        self.root.setHidden(true);
        // Let go of the pixels / document.
        self.image.setImage(None);
        if let Some(p) = self.pdf.get() {
            let none: Option<&AnyObject> = None;
            let _: () = unsafe { msg_send![&**p, setDocument: none] };
        }
    }

    /// The view to focus while media shows.
    pub fn focus_view(&self) -> Retained<NSView> {
        match self.pdf.get() {
            Some(p) if !p.isHidden() => p.clone(),
            _ => Retained::into_super(Retained::into_super(self.image.clone())),
        }
    }

    fn pdf_view(&self) -> Option<Retained<NSView>> {
        if let Some(v) = self.pdf.get() {
            return Some(v.clone());
        }
        let cls = AnyClass::get(c"PDFView")?;
        let content = self.root.contentView()?;
        let view: Retained<NSView> = unsafe {
            let v: Allocated<NSView> = msg_send![cls, alloc];
            let v: Retained<NSView> = msg_send![v, initWithFrame: content.bounds()];
            let _: () = msg_send![&*v, setAutoScales: true];
            let _: () = msg_send![&*v, setDisplayMode: PDF_SINGLE_PAGE_CONTINUOUS];
            let _: () = msg_send![&*v, setDisplaysPageBreaks: true];
            v
        };
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&view);
        let _ = self.pdf.set(view.clone());
        Some(view)
    }
}

trait Inset {
    fn inset(self, d: f64) -> Self;
}

impl Inset for NSRect {
    fn inset(self, d: f64) -> Self {
        NSRect::new(
            objc2_foundation::NSPoint::new(self.origin.x + d, self.origin.y + d),
            NSSize::new((self.size.width - 2.0 * d).max(0.0), (self.size.height - 2.0 * d).max(0.0)),
        )
    }
}

