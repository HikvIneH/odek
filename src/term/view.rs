//! TermView: draws a `Term` with AppKit string drawing (Core Text underneath)
//! in a plain NSView — no Metal, no glyph atlas of our own. Only rows that
//! changed are redrawn.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dispatch2::DispatchQueue;
use objc2::AnyThread;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSBezierPath, NSColor,
    NSCompositingOperation, NSCursor, NSEvent, NSEventModifierFlags, NSFont, NSFontAttributeName,
    NSFontManager, NSFontTraitMask, NSFontWeightRegular, NSForegroundColorAttributeName, NSPasteboard,
    NSPasteboardTypeString, NSRectFillUsingOperation, NSResponder, NSStrikethroughStyleAttributeName,
    NSStringDrawing, NSTextInputClient, NSTrackingArea, NSTrackingAreaOptions, NSUnderlineStyleAttributeName,
    NSView, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSNumber, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSString, NSUInteger,
    NSURL, NSUserDefaults,
};

use super::findbar::{self, FindState};
use super::grid::{Cell as GCell, Color, Line, Style, attr, flag};
use super::ime::{self, Ime};
use super::input::{self, Mods};
use super::links::{self, Target};
use super::session::{Session, Spawn};
use super::settings::{self, ThemePref};
use super::vt::{CursorShape, Event, MouseMode, Term};

const PAD_X: f64 = 10.0;
const PAD_Y: f64 = 6.0;
pub const DEFAULT_FONT_SIZE: f64 = 13.0;

type EventHandler = Box<dyn Fn(ViewEvent)>;

/// What a view tells its owner (window or pane container).
// Payloads not read yet are for desktop notifications and opening files at a line.
#[allow(dead_code)]
pub enum ViewEvent {
    Title(String),
    /// Bell or desktop notification from the program.
    Attention(Option<String>),
    Exited(i32),
    /// The view became first responder.
    Focused,
    /// A file path was ⌘-clicked; it exists. The owner decides how to open it.
    OpenPath {
        path: PathBuf,
        line: Option<u32>,
        col: Option<u32>,
    },
}

thread_local! {
    static VIEWS: RefCell<HashMap<u64, Weak<TermView>>> = RefCell::new(HashMap::new());
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Pos {
    /// Stable line id (see `Term::first_id`).
    line: u64,
    col: usize,
}

#[derive(Default)]
struct Hover {
    /// Cell under the mouse (line id, col) while ⌘ is held.
    key: Option<(u64, usize)>,
    /// The link under it: line id and cell range, underlined.
    span: Option<(u64, usize, usize)>,
    /// ⌘-click opened a link; ignore the rest of that click.
    swallow: bool,
}

#[derive(Clone, Copy)]
struct Metrics {
    cw: f64,
    ch: f64,
}

struct Theme {
    fg: u32,
    bg: u32,
    ansi: [u32; 16],
}

const CURSOR: u32 = 0x3B82F6;

const DARK: Theme = Theme {
    fg: 0xE6EDF3,
    // The icon's ink.
    bg: 0x0B0F14,
    ansi: [
        0x484F58, 0xFF7B72, 0x3FB950, 0xD29922, 0x58A6FF, 0xBC8CFF, 0x39C5CF, 0xB1BAC4, 0x6E7681, 0xFFA198,
        0x56D364, 0xE3B341, 0x79C0FF, 0xD2A8FF, 0x56D4DD, 0xFFFFFF,
    ],
};

const LIGHT: Theme = Theme {
    fg: 0x1F2328,
    bg: 0xFFFFFF,
    ansi: [
        0x24292F, 0xCF222E, 0x116329, 0x4D2D00, 0x0969DA, 0x8250DF, 0x1B7C83, 0x6E7781, 0x57606A, 0xA40E26,
        0x1A7F37, 0x633C01, 0x218BFF, 0xA475F9, 0x3192AA, 0x8C959F,
    ],
};

impl Theme {
    fn rgb(&self, c: Color, fg: bool) -> u32 {
        match c {
            Color::Default => {
                if fg {
                    self.fg
                } else {
                    self.bg
                }
            }
            Color::Indexed(i) if i < 16 => self.ansi[i as usize],
            Color::Indexed(i) if i < 232 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v as u32 };
                (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
            }
            Color::Indexed(i) => {
                let g = 8 + 10 * (i - 232) as u32;
                (g << 16) | (g << 8) | g
            }
            Color::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
        }
    }

    /// Final (fg, bg) of a style after inverse, hidden and dim.
    fn resolve(&self, s: &Style) -> (u32, u32) {
        let (mut fg, mut bg) = (self.rgb(s.fg, true), self.rgb(s.bg, false));
        if s.attrs & attr::INVERSE != 0 {
            std::mem::swap(&mut fg, &mut bg);
        }
        if s.attrs & attr::HIDDEN != 0 {
            fg = bg;
        } else if s.attrs & attr::DIM != 0 {
            fg = blend(fg, bg);
        }
        (fg, bg)
    }
}

fn blend(a: u32, b: u32) -> u32 {
    let ch = |s: u32| (((a >> s) & 0xFF) + ((b >> s) & 0xFF)) / 2;
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

fn rgb_tuple(c: u32) -> (u8, u8, u8) {
    ((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub struct Ivars {
    id: u64,
    session: RefCell<Option<Session>>,
    on_event: RefCell<Option<EventHandler>>,
    font_size: Cell<f64>,
    fonts: RefCell<[Retained<NSFont>; 4]>,
    metrics: Cell<Metrics>,
    dark: Cell<bool>,
    colors: RefCell<HashMap<u32, Retained<NSColor>>>,
    attrs: RefCell<HashMap<u16, Retained<NSDictionary<NSString, AnyObject>>>>,
    /// Stable id of the top visible line while scrolled back; None = follow output.
    anchor: Cell<Option<u64>>,
    scroll_px: Cell<f64>,
    selection: Cell<Option<(Pos, Pos)>>,
    /// Selecting by words (double-click) or lines (triple-click).
    select_unit: Cell<u8>,
    focused: Cell<bool>,
    cursor_row: Cell<usize>,
    exited: Cell<bool>,
    /// Background opacity from settings; below 1 the view isn't opaque.
    opacity: Cell<f64>,
    pub(super) find: RefCell<Option<FindState>>,
    hover: RefCell<Hover>,
    ime: Ime,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct TermView;

    impl TermView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            self.ivars().opacity.get() >= 1.0
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            self.set_focused(true);
            self.emit(ViewEvent::Focused);
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.set_focused(false);
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, rect: NSRect) {
            self.draw(rect);
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.fit_grid();
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            self.update_theme();
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::IBeamCursor());
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.update_hover(self.local_point(event), event.modifierFlags().contains(NSEventModifierFlags::Command));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.set_hover(None);
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            if let Some(w) = self.window() {
                let p = self.convertPoint_fromView(w.mouseLocationOutsideOfEventStream(), None);
                self.update_hover(p, event.modifierFlags().contains(NSEventModifierFlags::Command));
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.handle_key(event);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.handle_mouse(event, MouseKind::Down);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.handle_mouse(event, MouseKind::Drag);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.handle_mouse(event, MouseKind::Up);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.handle_scroll(event);
        }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            if let Some(text) = self.selected_text() {
                let pb = NSPasteboard::generalPasteboard();
                pb.clearContents();
                pb.setString_forType(&NSString::from_str(&text), unsafe { NSPasteboardTypeString });
            }
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            let pb = NSPasteboard::generalPasteboard();
            if let Some(s) = pb.stringForType(unsafe { NSPasteboardTypeString }) {
                self.paste_text(&s.to_string());
            }
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: Option<&AnyObject>) {
            let Some((first, total, cols)) = self.with_term(|t| (t.first_id(), t.total_lines(), t.cols)) else {
                return;
            };
            self.ivars().selection.set(Some((
                Pos { line: first, col: 0 },
                Pos { line: first + total as u64 - 1, col: cols },
            )));
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(termFind:))]
        fn term_find(&self, _sender: Option<&AnyObject>) {
            findbar::open(self);
        }

        #[unsafe(method(termFindNext:))]
        fn term_find_next(&self, _sender: Option<&AnyObject>) {
            findbar::step(self, 1);
        }

        #[unsafe(method(termFindPrevious:))]
        fn term_find_previous(&self, _sender: Option<&AnyObject>) {
            findbar::step(self, -1);
        }

        #[unsafe(method(clearScrollback:))]
        fn clear_scrollback(&self, _sender: Option<&AnyObject>) {
            // Like ⌘K in Terminal: drop scrollback, then ask the program to redraw.
            if let Some(s) = self.ivars().session.borrow().as_ref() {
                s.term.lock().unwrap().history.clear();
                s.write(vec![0x0c]);
            }
            self.ivars().anchor.set(None);
            self.ivars().selection.set(None);
            self.setNeedsDisplay(true);
        }
    }

    unsafe impl NSTextInputClient for TermView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, _range: NSRange) {
            self.commit_text(&ime::string_of(string));
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command(&self, _sel: objc2::runtime::Sel) {
            // Keys the input method doesn't take (and which aren't beeps): send as before.
            if let Some(event) = self.ivars().ime.take_event() {
                self.send_key(&event);
            }
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, string: &AnyObject, selected: NSRange, _range: NSRange) {
            self.mark_text(&ime::string_of(string), selected);
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            if self.ivars().ime.clear() {
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            self.ivars().ime.selected_range()
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            self.ivars().ime.marked_range()
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            self.ivars().ime.has_marked()
        }

        #[unsafe(method(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(
            &self,
            _range: NSRange,
            _actual: NSRangePointer,
        ) -> *mut AnyObject {
            std::ptr::null_mut()
        }

        #[unsafe(method(validAttributesForMarkedText))]
        fn valid_attributes(&self) -> *mut AnyObject {
            Retained::autorelease_ptr(NSArray::<NSString>::new()).cast()
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, _range: NSRange, _actual: NSRangePointer) -> NSRect {
            self.ime_screen_rect()
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, _point: NSPoint) -> NSUInteger {
            ime::NOT_FOUND
        }
    }
);

#[derive(Clone, Copy, PartialEq)]
enum MouseKind {
    Down,
    Drag,
    Up,
}

impl TermView {
    pub fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let size = settings::font_size();
        let fonts = make_fonts(size);
        let metrics = measure(&fonts[0]);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let this = Self::alloc(mtm).set_ivars(Ivars {
            id,
            session: RefCell::new(None),
            on_event: RefCell::new(None),
            font_size: Cell::new(size),
            fonts: RefCell::new(fonts),
            metrics: Cell::new(metrics),
            dark: Cell::new(true),
            colors: RefCell::new(HashMap::new()),
            attrs: RefCell::new(HashMap::new()),
            anchor: Cell::new(None),
            scroll_px: Cell::new(0.0),
            selection: Cell::new(None),
            select_unit: Cell::new(1),
            focused: Cell::new(false),
            cursor_row: Cell::new(0),
            exited: Cell::new(false),
            opacity: Cell::new(settings::opacity()),
            find: RefCell::new(None),
            hover: RefCell::new(Hover::default()),
            ime: Ime::default(),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        // Since macOS 14 views draw outside their bounds unless told not to;
        // the background fill would cover neighbours such as pane headers.
        // (Older macOS clips by default and lacks the setter.)
        let base: &NSView = &this;
        if objc2_foundation::NSObjectProtocol::respondsToSelector(base, objc2::sel!(setClipsToBounds:)) {
            this.setClipsToBounds(true);
        }
        VIEWS.with(|v| v.borrow_mut().insert(id, Weak::from_retained(&this)));
        this.update_theme();
        let opts = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                opts,
                Some(&*this),
                None,
            )
        };
        this.addTrackingArea(&area);
        this
    }

    pub fn set_on_event(&self, f: impl Fn(ViewEvent) + 'static) {
        *self.ivars().on_event.borrow_mut() = Some(Box::new(f));
    }

    /// Start the shell (or `command` through the login shell) in `cwd`.
    pub fn start(&self, cwd: &Path, command: Option<&str>) -> std::io::Result<()> {
        let (cols, rows) = self.grid_size();
        let id = self.ivars().id;
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            DispatchQueue::main().exec_async(move || {
                if let Some(view) = VIEWS.with(|v| v.borrow().get(&id).and_then(Weak::load)) {
                    view.pump();
                }
            });
        });
        let m = self.ivars().metrics.get();
        let opts = Spawn {
            cwd,
            command,
            cols,
            rows,
            cell_px: (m.cw.round() as u16, m.ch.round() as u16),
        };
        let session = Session::spawn(&opts, wake)?;
        {
            let mut t = session.term.lock().unwrap();
            let theme = self.theme();
            t.report_fg = rgb_tuple(theme.fg);
            t.report_bg = rgb_tuple(theme.bg);
            settings::apply_scrollback(&mut t);
        }
        *self.ivars().session.borrow_mut() = Some(session);
        self.setNeedsDisplay(true);
        Ok(())
    }

    /// Hang up the shell and free the terminal now (closing a pane).
    pub fn shutdown(&self) {
        self.ivars().session.borrow_mut().take();
        VIEWS.with(|v| v.borrow_mut().remove(&self.ivars().id));
    }

    pub fn session_cwd(&self) -> Option<std::path::PathBuf> {
        self.ivars().session.borrow().as_ref().and_then(Session::cwd)
    }

    pub fn foreground_name(&self) -> Option<String> {
        self.ivars()
            .session
            .borrow()
            .as_ref()
            .and_then(Session::foreground_name)
    }

    #[cfg_attr(not(feature = "selftest"), allow(dead_code))]
    pub fn mem_bytes(&self) -> usize {
        self.with_term(|t| t.mem_bytes()).unwrap_or(0)
    }

    #[cfg_attr(not(feature = "selftest"), allow(dead_code))]
    pub fn screen_text(&self) -> String {
        self.with_term(|t| t.screen_text()).unwrap_or_default()
    }

    pub fn write(&self, bytes: &[u8]) {
        if let Some(s) = self.ivars().session.borrow().as_ref() {
            s.write(bytes.to_vec());
        }
    }

    pub fn set_font_size(&self, size: f64) {
        let size = size.clamp(8.0, 36.0);
        self.ivars().font_size.set(size);
        let fonts = make_fonts(size);
        self.ivars().metrics.set(measure(&fonts[0]));
        *self.ivars().fonts.borrow_mut() = fonts;
        self.ivars().attrs.borrow_mut().clear();
        self.fit_grid();
        self.setNeedsDisplay(true);
    }

    /// Every live terminal.
    pub fn all() -> Vec<Retained<TermView>> {
        VIEWS.with(|v| v.borrow().values().filter_map(Weak::load).collect())
    }

    /// Re-read the saved font, size, theme and scrollback.
    pub fn apply_settings(&self) {
        self.ivars().opacity.set(settings::opacity());
        self.update_theme();
        self.set_font_size(settings::font_size());
        self.with_term_mut(settings::apply_scrollback);
    }

    fn with_term_mut(&self, f: impl FnOnce(&mut Term)) {
        if let Some(s) = self.ivars().session.borrow().as_ref() {
            f(&mut s.term.lock().unwrap());
        }
    }

    pub fn font_size(&self) -> f64 {
        self.ivars().font_size.get()
    }

    /// Point size of one cell, for sizing windows.
    pub fn cell_size(&self) -> (f64, f64) {
        let m = self.ivars().metrics.get();
        (m.cw, m.ch)
    }

    pub fn padding() -> (f64, f64) {
        (PAD_X, PAD_Y)
    }

    pub(super) fn with_term<R>(&self, f: impl FnOnce(&Term) -> R) -> Option<R> {
        let s = self.ivars().session.borrow();
        let s = s.as_ref()?;
        let t = s.term.lock().unwrap();
        Some(f(&t))
    }

    fn emit(&self, e: ViewEvent) {
        if let Some(f) = self.ivars().on_event.borrow().as_ref() {
            f(e);
        }
    }

    fn grid_size(&self) -> (u16, u16) {
        let b = self.bounds().size;
        let m = self.ivars().metrics.get();
        let cols = ((b.width - 2.0 * PAD_X) / m.cw).floor().max(2.0) as u16;
        let rows = ((b.height - 2.0 * PAD_Y) / m.ch).floor().max(1.0) as u16;
        (cols, rows)
    }

    fn fit_grid(&self) {
        let (cols, rows) = self.grid_size();
        let m = self.ivars().metrics.get();
        if let Some(s) = self.ivars().session.borrow().as_ref() {
            let same = {
                let t = s.term.lock().unwrap();
                t.cols == cols as usize && t.rows == rows as usize
            };
            if !same {
                s.resize(cols, rows, (m.cw.round() as u16, m.ch.round() as u16));
                if std::mem::take(&mut s.term.lock().unwrap().reflowed) {
                    self.ivars().anchor.set(None);
                }
                self.setNeedsDisplay(true);
            }
        }
    }

    fn theme(&self) -> &'static Theme {
        if self.ivars().dark.get() { &DARK } else { &LIGHT }
    }

    fn update_theme(&self) {
        let names = unsafe { NSArray::from_slice(&[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]) };
        let dark = match settings::theme() {
            ThemePref::Light => false,
            ThemePref::Dark => true,
            ThemePref::System => self
                .effectiveAppearance()
                .bestMatchFromAppearancesWithNames(&names)
                .is_some_and(|n| n.isEqualToString(unsafe { NSAppearanceNameDarkAqua })),
        };
        self.ivars().dark.set(dark);
        self.ivars().attrs.borrow_mut().clear();
        if let Some(s) = self.ivars().session.borrow().as_ref() {
            let theme = self.theme();
            let mut t = s.term.lock().unwrap();
            t.report_fg = rgb_tuple(theme.fg);
            t.report_bg = rgb_tuple(theme.bg);
        }
        self.setNeedsDisplay(true);
    }

    fn color(&self, rgb: u32) -> Retained<NSColor> {
        self.ivars()
            .colors
            .borrow_mut()
            .entry(rgb)
            .or_insert_with(|| {
                let c = |s: u32| ((rgb >> s) & 0xFF) as f64 / 255.0;
                NSColor::colorWithSRGBRed_green_blue_alpha(c(16), c(8), c(0), 1.0)
            })
            .clone()
    }

    fn attrs_for(&self, id: u16, style: &Style) -> Retained<NSDictionary<NSString, AnyObject>> {
        if let Some(a) = self.ivars().attrs.borrow().get(&id) {
            return a.clone();
        }
        let (fg, _) = self.theme().resolve(style);
        let fonts = self.ivars().fonts.borrow();
        let bold = style.attrs & attr::BOLD != 0;
        let italic = style.attrs & attr::ITALIC != 0;
        let font = &fonts[bold as usize + 2 * italic as usize];
        let color = self.color(fg);
        let one = NSNumber::new_isize(1);
        let mut keys: Vec<&NSString> = unsafe { vec![NSFontAttributeName, NSForegroundColorAttributeName] };
        let mut vals: Vec<&AnyObject> = vec![&**font, &*color];
        if style.attrs & attr::UNDERLINE != 0 {
            keys.push(unsafe { NSUnderlineStyleAttributeName });
            vals.push(&*one);
        }
        if style.attrs & attr::STRIKE != 0 {
            keys.push(unsafe { NSStrikethroughStyleAttributeName });
            vals.push(&*one);
        }
        let dict = NSDictionary::from_slices(&keys, &vals);
        self.ivars().attrs.borrow_mut().insert(id, dict.clone());
        dict
    }

    /// Index (into `0..total_lines`) of the top visible line.
    pub(super) fn top_index(&self, t: &Term) -> usize {
        let live = t.total_lines() - t.rows;
        match self.ivars().anchor.get() {
            Some(id) if !t.alt_active => (id.saturating_sub(t.first_id()) as usize).min(live),
            _ => live,
        }
    }

    // ---- output ----

    /// New output arrived: redraw what changed, pass events on.
    fn pump(&self) {
        let Some((events, all, dirty, cursor, exited)) = ({
            let s = self.ivars().session.borrow();
            s.as_ref().map(|s| {
                s.wake_pending.store(false, Ordering::SeqCst);
                let mut t = s.term.lock().unwrap();
                let events = std::mem::take(&mut t.events);
                let (all, dirty) = t.take_dirty();
                (
                    events,
                    all,
                    dirty,
                    t.cursor_pos().0,
                    s.exited.lock().unwrap().is_some(),
                )
            })
        }) else {
            return;
        };
        let anchored = self.ivars().anchor.get().is_some();
        if all || anchored {
            self.setNeedsDisplay(true);
        } else {
            let old = self.ivars().cursor_row.replace(cursor);
            let mut rows = dirty;
            for r in [old, cursor] {
                if let Some(d) = rows.get_mut(r) {
                    *d = true;
                }
            }
            let ch = self.ivars().metrics.get().ch;
            let width = self.bounds().size.width;
            let mut r = 0;
            while r < rows.len() {
                if !rows[r] {
                    r += 1;
                    continue;
                }
                let start = r;
                while r < rows.len() && rows[r] {
                    r += 1;
                }
                let rect = NSRect::new(
                    NSPoint::new(0.0, PAD_Y + start as f64 * ch),
                    NSSize::new(width, (r - start) as f64 * ch),
                );
                self.setNeedsDisplayInRect(rect);
            }
        }
        self.ivars().cursor_row.set(cursor);
        for e in events {
            match e {
                Event::Title(t) => self.emit(ViewEvent::Title(t)),
                Event::Bell => self.emit(ViewEvent::Attention(None)),
                Event::Notify(m) => self.emit(ViewEvent::Attention(Some(m))),
                Event::Clipboard(text) => {
                    let pb = NSPasteboard::generalPasteboard();
                    pb.clearContents();
                    pb.setString_forType(&NSString::from_str(&text), unsafe { NSPasteboardTypeString });
                }
                Event::Cwd(_) => {}
            }
        }
        if exited && !self.ivars().exited.replace(true) {
            let code = self
                .ivars()
                .session
                .borrow()
                .as_ref()
                .and_then(|s| *s.exited.lock().unwrap())
                .unwrap_or(0);
            self.setNeedsDisplay(true);
            self.emit(ViewEvent::Exited(code));
        }
    }

    // ---- drawing ----

    fn draw(&self, dirty: NSRect) {
        let theme = self.theme();
        // Copy, not blend: with opacity below 1 the background's alpha must
        // replace what was drawn before, not pile up.
        let opacity = self.ivars().opacity.get();
        let bg = self.color(theme.bg);
        if opacity < 1.0 {
            bg.colorWithAlphaComponent(opacity).setFill();
        } else {
            bg.setFill();
        }
        NSRectFillUsingOperation(dirty, NSCompositingOperation::Copy);
        let session = self.ivars().session.borrow();
        let Some(s) = session.as_ref() else { return };
        let t = s.term.lock().unwrap();
        let m = self.ivars().metrics.get();
        let top = self.top_index(&t);
        let r0 = ((dirty.origin.y - PAD_Y) / m.ch).floor().max(0.0) as usize;
        let r1 = (((dirty.origin.y + dirty.size.height - PAD_Y) / m.ch)
            .ceil()
            .max(0.0) as usize)
            .min(t.rows);
        let sel = self
            .ivars()
            .selection
            .get()
            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let first = t.first_id();
        for r in r0..r1 {
            let idx = top + r;
            if idx >= t.total_lines() {
                break;
            }
            let id = first + idx as u64;
            let span = sel.and_then(|(a, b)| {
                (a.line <= id && id <= b.line).then(|| {
                    let from = if id == a.line { a.col } else { 0 };
                    let to = if id == b.line { b.col } else { t.cols };
                    (from, to)
                })
            });
            self.draw_line(&t, t.line(idx), PAD_Y + r as f64 * m.ch, span);
            findbar::paint(self, id, |a, b| cell_rect(m, a, b - a, PAD_Y + r as f64 * m.ch));
            if let Some((l, a, b)) = self.ivars().hover.borrow().span
                && l == id
            {
                self.draw_link_underline(&t, t.line(idx), (a, b), PAD_Y + r as f64 * m.ch);
            }
        }
        let live = top + t.rows == t.total_lines();
        if live && t.modes.show_cursor && !self.ivars().exited.get() {
            self.draw_cursor(&t, m);
        }
        if live && let Some(text) = self.ivars().ime.marked() {
            let (row, col) = t.cursor_pos();
            let origin = NSPoint::new(PAD_X + col as f64 * m.cw, PAD_Y + row as f64 * m.ch);
            let (fg, bg) = (self.color(theme.fg), self.color(theme.bg));
            ime::draw_marked(
                &text,
                origin,
                m.cw,
                m.ch,
                t.cols.saturating_sub(col),
                &fg,
                &bg,
                &self.ivars().fonts.borrow()[0],
            );
        }
    }

    fn draw_line(&self, t: &Term, line: &Line, y: f64, sel: Option<(usize, usize)>) {
        let theme = self.theme();
        let m = self.ivars().metrics.get();
        let cells = &line.cells;
        let bg_of = |c: &GCell| theme.resolve(&t.styles.get(c.style)).1;

        let mut c = 0;
        while c < cells.len() {
            let bg = bg_of(&cells[c]);
            let start = c;
            while c < cells.len() && bg_of(&cells[c]) == bg {
                c += 1;
            }
            if bg != theme.bg {
                self.color(bg).setFill();
                NSBezierPath::fillRect(cell_rect(m, start, c - start, y));
            }
        }
        if let Some((from, to)) = sel
            && to > from
        {
            NSColor::selectedTextBackgroundColor().setFill();
            NSBezierPath::fillRect(cell_rect(m, from, to - from, y));
        }

        let scale = self.window().map_or(2.0, |w| w.backingScaleFactor());
        let mut buf = String::new();
        let mut c = 0;
        while c < cells.len() {
            let cell = cells[c];
            let style = t.styles.get(cell.style);
            let decorated = style.attrs & (attr::UNDERLINE | attr::STRIKE) != 0;
            if cell.flags & flag::SPACER != 0 || (cell.ch == ' ' as u32 && !decorated) {
                c += 1;
                continue;
            }
            let start = c;
            buf.clear();
            if cell.flags == 0 && cell.ch < 0x7f {
                // A run of plain ASCII in one style draws as one string.
                while c < cells.len()
                    && cells[c].flags == 0
                    && cells[c].ch < 0x7f
                    && cells[c].style == cell.style
                {
                    buf.push(cells[c].ch as u8 as char);
                    c += 1;
                }
                if !decorated {
                    let end = buf.trim_end_matches(' ').len();
                    buf.truncate(end);
                }
            } else {
                if cell.flags & flag::CLUSTER != 0 {
                    buf.push_str(t.clusters.get(cell.ch));
                } else {
                    buf.push(char::from_u32(cell.ch).unwrap_or(' '));
                }
                c += 1;
            }
            let x = PAD_X + start as f64 * m.cw;
            if cell.flags & flag::CLUSTER == 0 && super::boxdraw::is_drawn(cell.ch) {
                let (fg, _) = theme.resolve(&style);
                self.color(fg).setFill();
                self.color(fg).setStroke();
                super::boxdraw::draw(
                    cell.ch,
                    NSRect::new(NSPoint::new(x, y), NSSize::new(m.cw, m.ch)),
                    scale,
                );
                continue;
            }
            let attrs = self.attrs_for(cell.style, &style);
            unsafe { NSString::from_str(&buf).drawAtPoint_withAttributes(NSPoint::new(x, y), Some(&attrs)) };
        }
    }

    fn draw_link_underline(&self, t: &Term, line: &Line, (a, b): (usize, usize), y: f64) {
        let m = self.ivars().metrics.get();
        let (fg, _) = self
            .theme()
            .resolve(&t.styles.get(line.cells.get(a).map_or(0, |c| c.style)));
        self.color(fg).setFill();
        let r = cell_rect(m, a, b - a, y + m.ch - 2.0);
        NSBezierPath::fillRect(NSRect::new(r.origin, NSSize::new(r.size.width, 1.0)));
    }

    fn draw_cursor(&self, t: &Term, m: Metrics) {
        let (row, col) = t.cursor_pos();
        let line = &t.grid().lines[row];
        let cell = line.cells[col];
        let wide = if cell.flags & flag::WIDE != 0 { 2 } else { 1 };
        let y = PAD_Y + row as f64 * m.ch;
        let rect = cell_rect(m, col, wide, y);
        let theme = self.theme();
        // The icon's cursor blue.
        let color = self.color(CURSOR);
        if !self.ivars().focused.get() {
            color.setStroke();
            NSBezierPath::strokeRect(NSRect::new(
                NSPoint::new(rect.origin.x + 0.5, rect.origin.y + 0.5),
                NSSize::new(rect.size.width - 1.0, rect.size.height - 1.0),
            ));
            return;
        }
        color.setFill();
        match t.cursor_shape {
            CursorShape::Bar => {
                NSBezierPath::fillRect(NSRect::new(rect.origin, NSSize::new(2.0, rect.size.height)));
            }
            CursorShape::Underline => NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(rect.origin.x, rect.origin.y + rect.size.height - 2.0),
                NSSize::new(rect.size.width, 2.0),
            )),
            CursorShape::Block => {
                NSBezierPath::fillRect(rect);
                let text = if cell.flags & flag::CLUSTER != 0 {
                    t.clusters.get(cell.ch).to_string()
                } else {
                    char::from_u32(cell.ch).unwrap_or(' ').to_string()
                };
                if text != " " {
                    let style = t.styles.get(cell.style);
                    let fonts = self.ivars().fonts.borrow();
                    let font = &fonts[(style.attrs & attr::BOLD != 0) as usize];
                    let bg = self.color(theme.bg);
                    let attrs: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
                        NSDictionary::from_slices(
                            &[NSFontAttributeName, NSForegroundColorAttributeName],
                            &[&**font as &AnyObject, &*bg as &AnyObject],
                        )
                    };
                    unsafe {
                        NSString::from_str(&text).drawAtPoint_withAttributes(rect.origin, Some(&attrs))
                    };
                }
            }
        }
    }

    fn set_focused(&self, on: bool) {
        self.ivars().focused.set(on);
        if let Some(s) = self.ivars().session.borrow().as_ref()
            && s.term.lock().unwrap().modes.focus_events
        {
            s.write(if on {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            });
        }
        let ch = self.ivars().metrics.get().ch;
        let row = self.ivars().cursor_row.get();
        self.setNeedsDisplayInRect(NSRect::new(
            NSPoint::new(0.0, PAD_Y + row as f64 * ch),
            NSSize::new(self.bounds().size.width, ch),
        ));
    }

    // ---- input ----

    fn handle_key(&self, event: &NSEvent) {
        let ime = &self.ivars().ime;
        let chars = event.characters().map(|s| s.to_string()).unwrap_or_default();
        if ime::wants(
            &chars,
            event.modifierFlags(),
            ime.has_marked(),
            settings::option_as_meta(),
        ) && let Some(ctx) = self.inputContext()
        {
            ime.begin(event);
            let handled = ctx.handleEvent(event);
            if ime.take_event().is_none() || handled {
                return;
            }
        }
        self.send_key(event);
    }

    fn send_key(&self, event: &NSEvent) {
        let flags = event.modifierFlags();
        let mods = Mods {
            shift: flags.contains(NSEventModifierFlags::Shift),
            ctrl: flags.contains(NSEventModifierFlags::Control),
            alt: flags.contains(NSEventModifierFlags::Option),
            cmd: flags.contains(NSEventModifierFlags::Command),
        };
        let chars = event.characters().map(|s| s.to_string()).unwrap_or_default();
        let bare = event
            .charactersIgnoringModifiers()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let key = bare.chars().next().map_or(0, |c| c as u32);

        // Shift+PageUp/PageDown/Home/End scroll the view, not the program.
        if mods.shift && !mods.ctrl && !mods.alt {
            let rows = self.with_term(|t| t.rows).unwrap_or(1) as isize;
            match key {
                0xF72C => return self.scroll_lines(rows - 1),
                0xF72D => return self.scroll_lines(-(rows - 1)),
                0xF729 => return self.scroll_lines(isize::MAX / 2),
                0xF72B => return self.scroll_lines(-isize::MAX / 2),
                _ => {}
            }
        }
        let app_cursor = self.with_term(|t| t.modes.app_cursor).unwrap_or(false);
        match input::encode_key(&chars, &bare, mods, app_cursor) {
            Some(bytes) if !bytes.is_empty() => {
                NSCursor::setHiddenUntilMouseMoves(true);
                self.follow_output();
                self.write(&bytes);
            }
            _ => {
                if mods.cmd {
                    let _: () = unsafe { msg_send![super(self), keyDown: event] };
                }
            }
        }
    }

    /// Committed text from the input system (typing, dead keys, emoji picker, IME).
    pub fn commit_text(&self, text: &str) {
        self.ivars().ime.clear();
        NSCursor::setHiddenUntilMouseMoves(true);
        self.follow_output();
        self.write(text.as_bytes());
        self.setNeedsDisplay(true);
    }

    pub fn mark_text(&self, text: &str, selected: NSRange) {
        if text.is_empty() {
            self.ivars().ime.clear();
        } else {
            self.ivars().ime.set(text.to_string(), selected);
            self.follow_output();
        }
        self.setNeedsDisplay(true);
    }

    /// Cursor cell (at the caret within marked text) in screen coordinates, for the candidate window.
    fn ime_screen_rect(&self) -> NSRect {
        let m = self.ivars().metrics.get();
        let (row, col) = self.with_term(|t| t.cursor_pos()).unwrap_or((0, 0));
        let col = col + self.ivars().ime.caret_cols();
        let rect = NSRect::new(
            NSPoint::new(PAD_X + col as f64 * m.cw, PAD_Y + row as f64 * m.ch),
            NSSize::new(m.cw, m.ch),
        );
        match self.window() {
            Some(w) => w.convertRectToScreen(self.convertRect_toView(rect, None)),
            None => rect,
        }
    }

    fn paste_text(&self, text: &str) {
        let bracketed = self.with_term(|t| t.modes.bracketed_paste).unwrap_or(false);
        self.follow_output();
        self.write(&input::encode_paste(text, bracketed));
    }

    fn follow_output(&self) {
        if self.ivars().anchor.replace(None).is_some() {
            self.setNeedsDisplay(true);
        }
    }

    /// Scroll the view back (positive) or forward (negative) by lines.
    pub(super) fn scroll_lines(&self, n: isize) {
        let Some((first, total, rows, alt)) =
            self.with_term(|t| (t.first_id(), t.total_lines(), t.rows, t.alt_active))
        else {
            return;
        };
        if alt {
            return;
        }
        let live = (total - rows) as isize;
        let cur = match self.ivars().anchor.get() {
            Some(id) => (id.saturating_sub(first) as isize).min(live),
            None => live,
        };
        let top = (cur - n).clamp(0, live);
        self.ivars().anchor.set(if top >= live {
            None
        } else {
            Some(first + top as u64)
        });
        self.setNeedsDisplay(true);
    }

    fn handle_scroll(&self, event: &NSEvent) {
        let m = self.ivars().metrics.get();
        let dy = if event.hasPreciseScrollingDeltas() {
            event.scrollingDeltaY()
        } else {
            event.scrollingDeltaY() * m.ch
        };
        let acc = self.ivars().scroll_px.get() + dy;
        let lines = (acc / m.ch).trunc();
        self.ivars().scroll_px.set(acc - lines * m.ch);
        let n = lines as isize;
        if n == 0 {
            return;
        }
        let Some((alt, mouse, sgr, app_cursor)) =
            self.with_term(|t| (t.alt_active, t.modes.mouse, t.modes.mouse_sgr, t.modes.app_cursor))
        else {
            return;
        };
        if mouse != MouseMode::Off && sgr {
            let (col, row) = self.cell_at(self.local_point(event)).unwrap_or((0, 0));
            let button = if n > 0 { 64 } else { 65 };
            let mut bytes = Vec::new();
            for _ in 0..n.unsigned_abs().min(10) {
                bytes.extend(input::encode_mouse_sgr(button, col, row, true, Mods::default()));
            }
            self.write(&bytes);
        } else if alt {
            // Full-screen programs without mouse support get arrow keys.
            let seq: &[u8] = match (n > 0, app_cursor) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.write(&seq.repeat(n.unsigned_abs().min(20)));
        } else {
            self.scroll_lines(n);
        }
    }

    fn local_point(&self, event: &NSEvent) -> NSPoint {
        self.convertPoint_fromView(event.locationInWindow(), None)
    }

    /// (col, screen row) under a point, clamped to the grid.
    fn cell_at(&self, p: NSPoint) -> Option<(usize, usize)> {
        let m = self.ivars().metrics.get();
        let (cols, rows) = self.with_term(|t| (t.cols, t.rows))?;
        let col = ((p.x - PAD_X) / m.cw).round().clamp(0.0, cols as f64) as usize;
        let row = ((p.y - PAD_Y) / m.ch).floor().clamp(0.0, rows as f64 - 1.0) as usize;
        Some((col, row))
    }

    fn pos_at(&self, p: NSPoint) -> Option<Pos> {
        let (col, row) = self.cell_at(p)?;
        let (first, top) = self.with_term(|t| (t.first_id(), self.top_index(t)))?;
        Some(Pos {
            line: first + (top + row) as u64,
            col,
        })
    }

    fn handle_mouse(&self, event: &NSEvent, kind: MouseKind) {
        let p = self.local_point(event);
        let flags = event.modifierFlags();
        if self.link_click(p, flags.contains(NSEventModifierFlags::Command), kind) {
            return;
        }
        let Some((mouse, sgr)) = self.with_term(|t| (t.modes.mouse, t.modes.mouse_sgr)) else {
            return;
        };
        // Programs that want the mouse get it; Shift still selects.
        if mouse != MouseMode::Off && sgr && !flags.contains(NSEventModifierFlags::Shift) {
            if kind == MouseKind::Drag && mouse == MouseMode::Click {
                return;
            }
            if let Some((col, row)) = self.cell_at(p) {
                let mods = Mods {
                    ctrl: flags.contains(NSEventModifierFlags::Control),
                    alt: flags.contains(NSEventModifierFlags::Option),
                    ..Mods::default()
                };
                let button = if kind == MouseKind::Drag { 32 } else { 0 };
                let col = col.min(self.with_term(|t| t.cols - 1).unwrap_or(0));
                self.write(&input::encode_mouse_sgr(
                    button,
                    col,
                    row,
                    kind != MouseKind::Up,
                    mods,
                ));
            }
            return;
        }
        if kind == MouseKind::Drag {
            // Dragging past the edge scrolls.
            let h = self.bounds().size.height;
            if p.y < 0.0 {
                self.scroll_lines(1);
            } else if p.y > h {
                self.scroll_lines(-1);
            }
        }
        let Some(pos) = self.pos_at(p) else { return };
        match kind {
            MouseKind::Down => {
                let clicks = event.clickCount().clamp(1, 3) as u8;
                self.ivars().select_unit.set(clicks);
                let sel = match clicks {
                    2 => self.word_at(pos),
                    3 => self.line_span(pos),
                    _ => (pos, pos),
                };
                self.ivars().selection.set(Some(sel));
            }
            MouseKind::Drag => {
                if let Some((anchor, _)) = self.ivars().selection.get() {
                    let end = match self.ivars().select_unit.get() {
                        2 => {
                            let (a, b) = self.word_at(pos);
                            if pos < anchor { a } else { b }
                        }
                        3 => {
                            let (a, b) = self.line_span(pos);
                            if pos < anchor { a } else { b }
                        }
                        _ => pos,
                    };
                    self.ivars().selection.set(Some((anchor, end)));
                }
            }
            MouseKind::Up => {
                if let Some((a, b)) = self.ivars().selection.get()
                    && a == b
                {
                    self.ivars().selection.set(None);
                }
            }
        }
        self.setNeedsDisplay(true);
    }

    // ---- links ----

    /// (line id, col) of the cell under a point, if inside the grid.
    fn hover_cell(&self, p: NSPoint) -> Option<(u64, usize)> {
        let m = self.ivars().metrics.get();
        let (cols, rows, first, top) =
            self.with_term(|t| (t.cols, t.rows, t.first_id(), self.top_index(t)))?;
        let (x, y) = (p.x - PAD_X, p.y - PAD_Y);
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let (col, row) = ((x / m.cw) as usize, (y / m.ch) as usize);
        (col < cols && row < rows).then_some((first + (top + row) as u64, col))
    }

    /// The link at a cell: OSC 8 first, else detected in the line's text.
    fn link_at(&self, line: u64, col: usize) -> Option<(usize, usize, Target)> {
        let found = self.with_term(|t| {
            let idx = line.checked_sub(t.first_id())? as usize;
            if idx >= t.total_lines() {
                return None;
            }
            let line = t.line(idx);
            let id = t.styles.get(line.cells.get(col)?.style).link;
            if id == 0 {
                return links::detect(&line_chars(t, line), col);
            }
            let same = |c: usize| t.styles.get(line.cells[c].style).link == id;
            let (mut a, mut b) = (col, col + 1);
            while a > 0 && same(a - 1) {
                a -= 1;
            }
            while b < line.cells.len() && same(b) {
                b += 1;
            }
            Some((a, b, Target::Url(t.link(id)?.to_string())))
        })??;
        // Only paths that exist count, so hover never promises a dead link.
        match &found.2 {
            Target::Path { path, .. } => self.resolve_path(path).map(|_| found),
            Target::Url(_) => Some(found),
        }
    }

    fn resolve_path(&self, path: &str) -> Option<PathBuf> {
        let p = match path.strip_prefix("~/") {
            Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
            None => PathBuf::from(path),
        };
        let p = if p.is_absolute() {
            p
        } else {
            self.session_cwd()?.join(p)
        };
        p.exists().then_some(p)
    }

    fn update_hover(&self, p: NSPoint, cmd: bool) {
        self.set_hover(if cmd { self.hover_cell(p) } else { None });
    }

    fn set_hover(&self, key: Option<(u64, usize)>) {
        if self.ivars().hover.borrow().key == key {
            return;
        }
        let span = key.and_then(|(l, c)| self.link_at(l, c).map(|(a, b, _)| (l, a, b)));
        let old = {
            let mut h = self.ivars().hover.borrow_mut();
            h.key = key;
            std::mem::replace(&mut h.span, span)
        };
        if old.is_some() != span.is_some() {
            let cursor = if span.is_some() {
                NSCursor::pointingHandCursor()
            } else {
                NSCursor::IBeamCursor()
            };
            cursor.set();
        }
        for (l, ..) in old.into_iter().chain(span) {
            self.invalidate_line(l);
        }
    }

    fn invalidate_line(&self, id: u64) {
        let Some(row) = self.with_term(|t| id.checked_sub(t.first_id() + self.top_index(t) as u64)) else {
            return;
        };
        let Some(row) = row else { return };
        let m = self.ivars().metrics.get();
        self.setNeedsDisplayInRect(NSRect::new(
            NSPoint::new(0.0, PAD_Y + row as f64 * m.ch),
            NSSize::new(self.bounds().size.width, m.ch),
        ));
    }

    /// ⌘-click on a link opens it instead of selecting. True if the event was consumed.
    fn link_click(&self, p: NSPoint, cmd: bool, kind: MouseKind) -> bool {
        if kind != MouseKind::Down {
            let mut h = self.ivars().hover.borrow_mut();
            let swallow = h.swallow;
            h.swallow &= kind != MouseKind::Up;
            return swallow;
        }
        self.ivars().hover.borrow_mut().swallow = false;
        if !cmd {
            return false;
        }
        let Some((line, col)) = self.hover_cell(p) else {
            return false;
        };
        let Some((_, _, target)) = self.link_at(line, col) else {
            return false;
        };
        self.ivars().hover.borrow_mut().swallow = true;
        self.open_target(target);
        true
    }

    fn open_target(&self, target: Target) {
        match target {
            Target::Url(u) => {
                let Some(url) = NSURL::URLWithString(&NSString::from_str(&u)) else {
                    return;
                };
                if url.isFileURL() {
                    if let Some(path) = url.path() {
                        self.open_path(&path.to_string(), None, None);
                    }
                } else {
                    NSWorkspace::sharedWorkspace().openURL(&url);
                }
            }
            Target::Path { path, line, col } => self.open_path(&path, line, col),
        }
    }

    fn open_path(&self, path: &str, line: Option<u32>, col: Option<u32>) {
        if let Some(path) = self.resolve_path(path) {
            self.emit(ViewEvent::OpenPath { path, line, col });
        }
    }

    /// Selftest: describe the link at a cell, optionally showing the ⌘-hover underline.
    #[cfg(feature = "selftest")]
    pub fn probe_link(&self, col: usize, row: usize, hover: bool) -> String {
        let key = self.with_term(|t| (t.first_id() + (self.top_index(t) + row) as u64, col));
        let found = key.and_then(|(l, c)| self.link_at(l, c));
        if hover {
            self.set_hover(key);
        }
        format!("{found:?}")
    }

    fn line_span(&self, pos: Pos) -> (Pos, Pos) {
        let cols = self.with_term(|t| t.cols).unwrap_or(0);
        (
            Pos {
                line: pos.line,
                col: 0,
            },
            Pos {
                line: pos.line,
                col: cols,
            },
        )
    }

    /// Word around a position: letters, digits and path characters.
    fn word_at(&self, pos: Pos) -> (Pos, Pos) {
        let chars: Vec<char> = self
            .with_term(|t| {
                let idx = pos.line.checked_sub(t.first_id())? as usize;
                (idx < t.total_lines()).then(|| line_chars(t, t.line(idx)))
            })
            .flatten()
            .unwrap_or_default();
        let is_word = |c: char| c.is_alphanumeric() || "_-./~:@%+#=".contains(c);
        let at = pos.col.min(chars.len().saturating_sub(1));
        if chars.get(at).is_none_or(|&c| !is_word(c)) {
            return (
                pos,
                Pos {
                    line: pos.line,
                    col: (pos.col + 1).min(chars.len()),
                },
            );
        }
        let mut a = at;
        while a > 0 && is_word(chars[a - 1]) {
            a -= 1;
        }
        let mut b = at;
        while b < chars.len() && is_word(chars[b]) {
            b += 1;
        }
        (
            Pos {
                line: pos.line,
                col: a,
            },
            Pos {
                line: pos.line,
                col: b,
            },
        )
    }

    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.ivars().selection.get()?;
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        self.with_term(|t| {
            let mut out = String::new();
            for id in a.line..=b.line {
                let Some(idx) = id.checked_sub(t.first_id()).map(|i| i as usize) else {
                    continue;
                };
                if idx >= t.total_lines() {
                    break;
                }
                let line = t.line(idx);
                let from = if id == a.line { a.col } else { 0 };
                let to = if id == b.line { b.col } else { line.cells.len() };
                let mut s = String::new();
                for c in line.cells.iter().take(to).skip(from) {
                    if c.flags & flag::SPACER != 0 {
                        continue;
                    }
                    if c.flags & flag::CLUSTER != 0 {
                        s.push_str(t.clusters.get(c.ch));
                    } else {
                        s.push(char::from_u32(c.ch).unwrap_or(' '));
                    }
                }
                let soft = line.wrapped && id != b.line && to >= line.cells.len();
                if !soft {
                    let end = s.trim_end().len();
                    s.truncate(end);
                }
                out.push_str(&s);
                if id != b.line && !soft {
                    out.push('\n');
                }
            }
            out
        })
        .filter(|s| !s.is_empty())
    }
}

/// One char per cell ('\0' for wide-char spacers), for word bounds and copying.
/// A multi-code-point cluster contributes its first char.
fn line_chars(t: &Term, line: &Line) -> Vec<char> {
    line.cells
        .iter()
        .map(|c| {
            if c.flags & flag::SPACER != 0 {
                '\0'
            } else if c.flags & flag::CLUSTER != 0 {
                t.clusters.get(c.ch).chars().next().unwrap_or(' ')
            } else {
                char::from_u32(c.ch).unwrap_or(' ')
            }
        })
        .collect()
}

/// Cells `col..col+n` on the row at `y`, edges on whole half-points so
/// adjacent runs meet without seams.
fn cell_rect(m: Metrics, col: usize, n: usize, y: f64) -> NSRect {
    let snap = |v: f64| (v * 2.0).round() / 2.0;
    let x0 = snap(PAD_X + col as f64 * m.cw);
    let x1 = snap(PAD_X + (col + n) as f64 * m.cw);
    NSRect::new(NSPoint::new(x0, y), NSSize::new(x1 - x0, m.ch))
}

/// Fonts tried in order when the user hasn't picked one: Nerd Font variants
/// first, so prompt themes (powerlevel10k etc.) get their icons.
const PREFERRED_FONTS: &[&str] = &[
    "MesloLGS NF",
    "MesloLGS Nerd Font Mono",
    "JetBrainsMono Nerd Font Mono",
    "Hack Nerd Font Mono",
];

/// [regular, bold, italic, bold italic]
fn make_fonts(size: f64) -> [Retained<NSFont>; 4] {
    let chosen = NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str("terminalFont"))
        .map(|s| s.to_string());
    let fm = NSFontManager::sharedFontManager(MainThreadMarker::new().expect("main thread"));
    // The settings window stores family names; fontWithName wants a face name.
    let by_family = |name: &str| {
        fm.fontWithFamily_traits_weight_size(&NSString::from_str(name), NSFontTraitMask::empty(), 5, size)
    };
    let regular = chosen
        .iter()
        .map(String::as_str)
        .filter(|n| !n.is_empty())
        .find_map(|name| {
            NSFont::fontWithName_size(&NSString::from_str(name), size).or_else(|| by_family(name))
        })
        .or_else(|| {
            PREFERRED_FONTS
                .iter()
                .find_map(|name| NSFont::fontWithName_size(&NSString::from_str(name), size))
        })
        .unwrap_or_else(|| NSFont::monospacedSystemFontOfSize_weight(size, unsafe { NSFontWeightRegular }));
    let bold = fm.convertFont_toHaveTrait(&regular, NSFontTraitMask::BoldFontMask);
    let italic = fm.convertFont_toHaveTrait(&regular, NSFontTraitMask::ItalicFontMask);
    let bold_italic = fm.convertFont_toHaveTrait(&bold, NSFontTraitMask::ItalicFontMask);
    [regular, bold, italic, bold_italic]
}

fn measure(font: &NSFont) -> Metrics {
    let attrs: Retained<NSDictionary<NSString, AnyObject>> =
        unsafe { NSDictionary::from_slices(&[NSFontAttributeName], &[font as &AnyObject]) };
    let size = unsafe { NSString::from_str("W").sizeWithAttributes(Some(&attrs)) };
    Metrics {
        cw: size.width,
        ch: size.height.ceil(),
    }
}
