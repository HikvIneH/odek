//! The profile picker (⇧⌘O): saved profiles on the left, what the selected
//! one opens on the right. Type to filter, ↑↓ to choose, Return to open (⌥
//! Return for another copy of a group that's already open), Esc to close.
//! Built when shown and dropped when it closes or loses focus, so it holds no
//! memory (window buffer, profiles) the rest of the time.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBezierPath, NSColor, NSColorSpace, NSEvent, NSEventModifierFlags,
    NSFont, NSFontAttributeName, NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName,
    NSLineBreakMode, NSMutableParagraphStyle, NSPanel, NSParagraphStyleAttributeName, NSResponder,
    NSStringDrawing, NSView, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSDictionary, NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use super::profile::Profile;

const WIDTH: f64 = 620.0;
const HEIGHT: f64 = 360.0;
const LIST_W: f64 = 210.0;
const PAD: f64 = 14.0;
const TOP: f64 = 46.0;
const ROW_H: f64 = 26.0;
const LINE_H: f64 = 18.0;
const FOOT_H: f64 = 30.0;

type OnPick = Rc<dyn Fn(Profile, bool)>;

thread_local! {
    static PANEL: RefCell<Option<(Retained<NSPanel>, Retained<PanelDelegate>)>> = const { RefCell::new(None) };
    static ON_PICK: RefCell<Option<OnPick>> = const { RefCell::new(None) };
}

pub struct PickerIvars {
    profiles: Vec<Profile>,
    /// Files that didn't parse: (file name, why).
    errors: Vec<(String, String)>,
    query: RefCell<String>,
    /// Indices into `profiles` that match the query.
    shown: RefCell<Vec<usize>>,
    selected: Cell<usize>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = PickerIvars]
    struct Picker;

    impl Picker {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _rect: NSRect) {
            self.draw();
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !self.key(event) {
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            if p.x >= LIST_W || p.y < TOP {
                return;
            }
            let row = ((p.y - TOP) / ROW_H) as usize;
            if row < self.ivars().shown.borrow().len() {
                self.ivars().selected.set(row);
                self.setNeedsDisplay(true);
                if event.clickCount() == 2 {
                    let another = event.modifierFlags().contains(NSEventModifierFlags::Option);
                    self.pick(another);
                }
            }
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            close_later();
        }
    }
);

impl Picker {
    fn new(mtm: MainThreadMarker, profiles: Vec<Profile>, errors: Vec<(String, String)>) -> Retained<Self> {
        let shown = (0..profiles.len()).collect();
        let this = Self::alloc(mtm).set_ivars(PickerIvars {
            profiles,
            errors,
            query: RefCell::new(String::new()),
            shown: RefCell::new(shown),
            selected: Cell::new(0),
        });
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Handle a key; false to pass it on (⌘-shortcuts and the like).
    fn key(&self, event: &NSEvent) -> bool {
        let flags = event.modifierFlags();
        if flags.contains(NSEventModifierFlags::Command) {
            return false;
        }
        let iv = self.ivars();
        match event.keyCode() {
            53 => close_later(),
            36 | 76 => self.pick(flags.contains(NSEventModifierFlags::Option)),
            125 => {
                let n = iv.shown.borrow().len();
                if n > 0 {
                    iv.selected.set((iv.selected.get() + 1).min(n - 1));
                }
            }
            126 => iv.selected.set(iv.selected.get().saturating_sub(1)),
            51 => {
                iv.query.borrow_mut().pop();
                self.refilter();
            }
            _ => {
                let typed = event.characters().map(|s| s.to_string()).unwrap_or_default();
                let typed: String = typed.chars().filter(|c| !c.is_control()).collect();
                if typed.is_empty() {
                    return false;
                }
                iv.query.borrow_mut().push_str(&typed);
                self.refilter();
            }
        }
        self.setNeedsDisplay(true);
        true
    }

    /// Keep the profiles whose name has the query's letters in order.
    fn refilter(&self) {
        let iv = self.ivars();
        let query = iv.query.borrow().to_lowercase();
        *iv.shown.borrow_mut() = iv
            .profiles
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                let mut name = p.name.to_lowercase().chars().collect::<Vec<_>>().into_iter();
                query
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .all(|q| name.any(|c| c == q))
            })
            .map(|(i, _)| i)
            .collect();
        iv.selected.set(0);
    }

    fn current(&self) -> Option<&Profile> {
        let iv = self.ivars();
        let i = *iv.shown.borrow().get(iv.selected.get())?;
        iv.profiles.get(i)
    }

    /// Open the selected profile once this event is done (the panel, and
    /// this view, go away first).
    fn pick(&self, another: bool) {
        let Some(p) = self.current().cloned() else { return };
        DispatchQueue::main().exec_async(move || {
            let f = ON_PICK.with(|f| f.borrow_mut().take());
            close();
            if let Some(f) = f {
                f(p, another);
            }
        });
    }

    fn draw(&self) {
        let iv = self.ivars();
        let label = NSColor::labelColor();
        let secondary = NSColor::secondaryLabelColor();
        let tertiary = NSColor::tertiaryLabelColor();

        // Filter line.
        let query = iv.query.borrow();
        let (text, color) = if query.is_empty() {
            ("Type to filter profiles".to_string(), &tertiary)
        } else {
            (query.clone(), &label)
        };
        text_at(
            &text,
            NSRect::new(NSPoint::new(PAD, 13.0), NSSize::new(WIDTH - 2.0 * PAD, 20.0)),
            14.0,
            false,
            color,
        );
        NSColor::separatorColor().setFill();
        NSBezierPath::fillRect(NSRect::new(NSPoint::new(0.0, TOP - 8.0), NSSize::new(WIDTH, 1.0)));
        NSBezierPath::fillRect(NSRect::new(
            NSPoint::new(LIST_W, TOP - 8.0),
            NSSize::new(1.0, HEIGHT - TOP + 8.0 - FOOT_H),
        ));
        NSBezierPath::fillRect(NSRect::new(
            NSPoint::new(0.0, HEIGHT - FOOT_H),
            NSSize::new(WIDTH, 1.0),
        ));

        if iv.profiles.is_empty() {
            let mut lines = vec![
                "No profiles yet.".to_string(),
                String::new(),
                "Set up a group's tabs and folders, then right-click".to_string(),
                "the group in the sidebar and choose".to_string(),
                "Save Group as Profile…".to_string(),
                String::new(),
                "Or write one by hand: Shell ▸ Edit Profiles…".to_string(),
            ];
            self.errors_into(&mut lines);
            self.lines(&lines, PAD, &secondary);
        } else {
            // The list.
            let shown = iv.shown.borrow();
            let sel = iv.selected.get();
            for (row, &i) in shown.iter().enumerate() {
                let y = TOP + row as f64 * ROW_H;
                if y + ROW_H > HEIGHT - FOOT_H {
                    break;
                }
                let p = &iv.profiles[i];
                let selected = row == sel;
                if selected {
                    NSColor::selectedContentBackgroundColor().setFill();
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                        NSRect::new(NSPoint::new(6.0, y), NSSize::new(LIST_W - 12.0, ROW_H - 2.0)),
                        5.0,
                        5.0,
                    )
                    .fill();
                }
                let fg = if selected {
                    NSColor::alternateSelectedControlTextColor()
                } else {
                    label.clone()
                };
                let mark = if p.open_at_launch { "  ⏻" } else { "" };
                text_at(
                    &format!("{}{mark}", p.name),
                    NSRect::new(NSPoint::new(PAD, y + 4.0), NSSize::new(LIST_W - 2.0 * PAD, 18.0)),
                    13.0,
                    false,
                    &fg,
                );
            }
            if shown.is_empty() {
                text_at(
                    "No profile matches",
                    NSRect::new(
                        NSPoint::new(PAD, TOP + 4.0),
                        NSSize::new(LIST_W - 2.0 * PAD, 18.0),
                    ),
                    13.0,
                    false,
                    &secondary,
                );
            }
            drop(shown);
            // The preview.
            let mut lines = self.current().map(Profile::preview).unwrap_or_default();
            if self.current().is_some_and(|p| p.open_at_launch) {
                lines.push(String::new());
                lines.push("⏻ opens when odek starts".into());
            }
            self.errors_into(&mut lines);
            self.lines(&lines, LIST_W + PAD, &label);
        }

        text_at(
            "↩ open    ⌥↩ open another copy    esc close",
            NSRect::new(
                NSPoint::new(PAD, HEIGHT - FOOT_H + 8.0),
                NSSize::new(WIDTH - 2.0 * PAD, 16.0),
            ),
            11.0,
            false,
            &secondary,
        );
    }

    fn errors_into(&self, lines: &mut Vec<String>) {
        let errors = &self.ivars().errors;
        if !errors.is_empty() {
            lines.push(String::new());
            lines.push("Couldn't read:".into());
            for (file, why) in errors {
                lines.push(format!("  {file}: {why}"));
            }
        }
    }

    /// Preview lines from the top of the right-hand side: tab titles in
    /// semibold, folders and commands in monospace.
    fn lines(&self, lines: &[String], x: f64, color: &NSColor) {
        let secondary = NSColor::secondaryLabelColor();
        let width = WIDTH - x - PAD;
        for (i, line) in lines.iter().enumerate() {
            let y = TOP + i as f64 * LINE_H;
            if y + LINE_H > HEIGHT - FOOT_H {
                break;
            }
            let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(width, LINE_H));
            if line.starts_with(' ') {
                mono_at(line.trim_start(), rect, line.starts_with("    ↳"), &secondary);
            } else {
                text_at(line, rect, 13.0, x > PAD, color);
            }
        }
    }
}

fn attrs(font: &NSFont, color: &NSColor) -> Retained<NSDictionary<NSString, AnyObject>> {
    let para = NSMutableParagraphStyle::new();
    para.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    unsafe {
        NSDictionary::from_slices(
            &[
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
            ],
            &[font as &AnyObject, color as &AnyObject, &*para as &AnyObject],
        )
    }
}

fn text_at(text: &str, rect: NSRect, size: f64, bold: bool, color: &NSColor) {
    let font = if bold {
        NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightSemibold })
    } else {
        NSFont::systemFontOfSize(size)
    };
    unsafe { NSString::from_str(text).drawInRect_withAttributes(rect, Some(&attrs(&font, color))) };
}

/// A folder (indented once) or a command (indented twice, after ↳).
fn mono_at(text: &str, rect: NSRect, command: bool, color: &NSColor) {
    let font = NSFont::monospacedSystemFontOfSize_weight(11.5, unsafe { NSFontWeightRegular });
    let indent = if command { 28.0 } else { 12.0 };
    let rect = NSRect::new(
        NSPoint::new(rect.origin.x + indent, rect.origin.y + 1.0),
        NSSize::new(rect.size.width - indent, rect.size.height),
    );
    let label = NSColor::labelColor();
    let color = if command { color } else { &label };
    unsafe { NSString::from_str(text).drawInRect_withAttributes(rect, Some(&attrs(&font, color))) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct PanelDelegate;

    unsafe impl NSObjectProtocol for PanelDelegate {}

    unsafe impl NSWindowDelegate for PanelDelegate {
        /// Clicking back into odek (or anywhere else) dismisses the picker.
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _n: &NSNotification) {
            close_later();
        }
    }
);

impl PanelDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// Show the picker over the key window. `on_pick` gets the chosen profile and
/// whether another copy was asked for.
pub fn show(
    mtm: MainThreadMarker,
    profiles: Vec<Profile>,
    errors: Vec<(String, String)>,
    on_pick: impl Fn(Profile, bool) + 'static,
) {
    close();
    let panel = new_panel(mtm);
    let delegate = PanelDelegate::new(mtm);
    let view = Picker::new(mtm, profiles, errors);
    panel.setContentView(Some(&view));
    panel.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let app = NSApplication::sharedApplication(mtm);
    if let Some(w) = app.keyWindow().or_else(|| app.mainWindow()) {
        let f = w.frame();
        panel.setFrameTopLeftPoint(NSPoint::new(
            f.origin.x + (f.size.width - WIDTH) / 2.0,
            f.origin.y + f.size.height - 90.0,
        ));
    } else {
        panel.center();
    }
    ON_PICK.with(|f| *f.borrow_mut() = Some(Rc::new(on_pick)));
    panel.makeKeyAndOrderFront(None);
    panel.makeFirstResponder(Some(&view));
    PANEL.with(|p| *p.borrow_mut() = Some((panel, delegate)));
}

fn new_panel(mtm: MainThreadMarker) -> Retained<NSPanel> {
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
        NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView,
        NSBackingStoreType::Buffered,
        false,
    );
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setTitle(&NSString::from_str("Open Profile"));
    panel.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden);
    panel.setTitlebarAppearsTransparent(true);
    // 8-bit buffers, like the main window (half the memory of the default).
    panel.setColorSpace(Some(&NSColorSpace::sRGBColorSpace()));
    panel
}

/// Close the picker and drop it: the panel, its buffer, every profile.
pub fn close() {
    ON_PICK.with(|f| f.borrow_mut().take());
    if let Some((panel, _delegate)) = PANEL.with(|p| p.borrow_mut().take()) {
        panel.setDelegate(None);
        panel.close();
    }
}

/// Selftest: the picker's content, to snapshot it.
#[cfg(feature = "selftest")]
pub fn content_view() -> Option<Retained<NSView>> {
    PANEL.with(|p| p.borrow().as_ref().and_then(|(panel, _)| panel.contentView()))
}

/// Close once the current event is done.
fn close_later() {
    DispatchQueue::main().exec_async(close);
}
