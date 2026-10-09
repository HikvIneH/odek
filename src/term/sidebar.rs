//! The sidebar: a search field, a + button and a hand-drawn list of tab
//! groups. It only reports what the user did; the workspace window owns the
//! model and pushes fresh rows back with `set_rows`.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSButton, NSColor, NSEvent, NSFont, NSFontAttributeName,
    NSFontWeightSemibold, NSForegroundColorAttributeName, NSImage, NSLineBreakMode, NSMenu,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSResponder, NSScrollView, NSSearchField,
    NSStringDrawing, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectView,
};
use objc2_foundation::{NSDictionary, NSObject, NSPoint, NSRect, NSSize, NSString};

use super::target::Target;
use super::workspace::Id;

const GROUP_H: f64 = 28.0;
const TAB_H: f64 = 44.0;
const TOP_BAR: f64 = 40.0;

#[derive(Clone, Debug)]
pub enum Row {
    Group {
        id: Id,
        name: String,
        collapsed: bool,
        count: usize,
    },
    Tab {
        id: Id,
        title: String,
        subtitle: String,
        active: bool,
        attention: bool,
        running: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowKey {
    Group(Id),
    Tab(Id),
    Empty,
}

pub enum SidebarEvent {
    Select(Id),
    ToggleGroup(Id),
    RenameTab(Id),
    RenameGroup(Id),
    Move { tab: Id, group: Id, index: usize },
    NewTab,
}

type Handler = Box<dyn Fn(SidebarEvent)>;
type MenuFor = Box<dyn Fn(RowKey) -> Option<Retained<NSMenu>>>;

#[derive(Clone, Copy)]
struct Drag {
    tab: Id,
    start: NSPoint,
    moving: bool,
    /// (group, index, y of the insertion line)
    drop: Option<(Id, usize, f64)>,
}

pub struct ListIvars {
    rows: RefCell<Vec<Row>>,
    query: RefCell<String>,
    /// Rows on screen after collapsing and filtering, with their top y.
    shown: RefCell<Vec<(Row, f64)>>,
    on_event: RefCell<Option<Handler>>,
    menu_for: RefCell<Option<MenuFor>>,
    drag: Cell<Option<Drag>>,
    pressed: Cell<Option<RowKey>>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ListIvars]
    pub struct SidebarList;

    impl SidebarList {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _rect: NSRect) {
            self.draw();
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let key = self.key_at(p.y);
            self.ivars().pressed.set(Some(key));
            match key {
                RowKey::Tab(id) => {
                    if event.clickCount() == 2 {
                        return self.emit(SidebarEvent::RenameTab(id));
                    }
                    self.ivars().drag.set(Some(Drag { tab: id, start: p, moving: false, drop: None }));
                    self.emit(SidebarEvent::Select(id));
                }
                RowKey::Group(id) if event.clickCount() == 2 => self.emit(SidebarEvent::RenameGroup(id)),
                _ => {}
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let Some(mut drag) = self.ivars().drag.get() else { return };
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            if !drag.moving && ((p.x - drag.start.x).abs() + (p.y - drag.start.y).abs()) < 5.0 {
                return;
            }
            drag.moving = true;
            drag.drop = self.drop_at(p.y);
            self.ivars().drag.set(Some(drag));
            self.autoscroll(event);
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let drag = self.ivars().drag.take();
            let pressed = self.ivars().pressed.take();
            if let Some(Drag { tab, moving: true, drop: Some((group, index, _)), .. }) = drag {
                self.setNeedsDisplay(true);
                return self.emit(SidebarEvent::Move { tab, group, index });
            }
            self.setNeedsDisplay(true);
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            if let Some(RowKey::Group(id)) = pressed
                && self.key_at(p.y) == RowKey::Group(id)
                && event.clickCount() == 1
            {
                self.emit(SidebarEvent::ToggleGroup(id));
            }
        }

        #[unsafe(method(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> *mut NSMenu {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let key = self.key_at(p.y);
            let menu = self.ivars().menu_for.borrow().as_ref().and_then(|f| f(key));
            menu.map_or(std::ptr::null_mut(), Retained::autorelease_return)
        }
    }
);

impl SidebarList {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ListIvars {
            rows: RefCell::new(Vec::new()),
            query: RefCell::new(String::new()),
            shown: RefCell::new(Vec::new()),
            on_event: RefCell::new(None),
            menu_for: RefCell::new(None),
            drag: Cell::new(None),
            pressed: Cell::new(None),
        });
        unsafe {
            msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(220.0, 100.0))]
        }
    }

    fn emit(&self, e: SidebarEvent) {
        if let Some(f) = self.ivars().on_event.borrow().as_ref() {
            f(e);
        }
    }

    /// Recompute which rows show (collapsed groups, search) and resize.
    fn relayout(&self) {
        let query = self.ivars().query.borrow().to_lowercase();
        let rows = self.ivars().rows.borrow();
        let mut shown = Vec::new();
        let mut y = 4.0;
        let mut i = 0;
        while i < rows.len() {
            let Row::Group { collapsed, name, .. } = &rows[i] else {
                i += 1;
                continue;
            };
            let group_hit = !query.is_empty() && name.to_lowercase().contains(&query);
            let mut tabs = Vec::new();
            i += 1;
            while let Some(row @ Row::Tab { title, subtitle, .. }) = rows.get(i) {
                let hit = query.is_empty()
                    || group_hit
                    || title.to_lowercase().contains(&query)
                    || subtitle.to_lowercase().contains(&query);
                if hit {
                    tabs.push(row.clone());
                }
                i += 1;
            }
            if !query.is_empty() && tabs.is_empty() {
                continue;
            }
            let group_row = rows[shown_group_index(&rows, i)].clone();
            shown.push((group_row, y));
            y += GROUP_H;
            if !*collapsed || !query.is_empty() {
                for t in tabs {
                    shown.push((t, y));
                    y += TAB_H;
                }
            }
        }
        drop(rows);
        *self.ivars().shown.borrow_mut() = shown;
        let parent = unsafe { self.superview() };
        let width = parent
            .as_ref()
            .map_or(self.frame().size.width, |s| s.bounds().size.width);
        let height = (y + 8.0).max(parent.as_ref().map_or(0.0, |s| s.bounds().size.height));
        self.setFrameSize(NSSize::new(width, height));
        self.setNeedsDisplay(true);
    }

    fn key_at(&self, y: f64) -> RowKey {
        for (row, top) in self.ivars().shown.borrow().iter() {
            let h = row_height(row);
            if y >= *top && y < top + h {
                return match row {
                    Row::Group { id, .. } => RowKey::Group(*id),
                    Row::Tab { id, .. } => RowKey::Tab(*id),
                };
            }
        }
        RowKey::Empty
    }

    /// Where a dragged tab would land: (group, index, line y).
    fn drop_at(&self, y: f64) -> Option<(Id, usize, f64)> {
        let shown = self.ivars().shown.borrow();
        let mut best = None;
        let mut index = 0;
        let mut group = None;
        for (row, top) in shown.iter() {
            match row {
                Row::Group { id, .. } => {
                    group = Some(*id);
                    index = 0;
                    if y >= *top && y < top + GROUP_H {
                        return Some((*id, 0, top + GROUP_H));
                    }
                }
                Row::Tab { .. } => {
                    let g = group?;
                    if y < top + TAB_H / 2.0 && y >= *top - TAB_H / 2.0 {
                        return Some((g, index, *top));
                    }
                    index += 1;
                    best = Some((g, index, top + TAB_H));
                }
            }
        }
        best
    }

    fn draw(&self) {
        let shown = self.ivars().shown.borrow();
        let width = self.bounds().size.width;
        let drag = self.ivars().drag.get();
        for (row, top) in shown.iter() {
            match row {
                Row::Group {
                    name,
                    collapsed,
                    count,
                    ..
                } => {
                    let chevron = if *collapsed { "▸" } else { "▾" };
                    draw_text(
                        chevron,
                        NSRect::new(NSPoint::new(10.0, top + 8.0), NSSize::new(12.0, 14.0)),
                        9.0,
                        false,
                        &NSColor::tertiaryLabelColor(),
                    );
                    let label = if *collapsed {
                        format!("{name}  ·  {count}")
                    } else {
                        name.clone()
                    };
                    draw_text(
                        &label,
                        NSRect::new(NSPoint::new(24.0, top + 7.0), NSSize::new(width - 34.0, 16.0)),
                        11.5,
                        true,
                        &NSColor::secondaryLabelColor(),
                    );
                }
                Row::Tab {
                    id,
                    title,
                    subtitle,
                    active,
                    attention,
                    running,
                    ..
                } => {
                    let card = NSRect::new(
                        NSPoint::new(8.0, top + 2.0),
                        NSSize::new(width - 16.0, TAB_H - 4.0),
                    );
                    let dragged = drag.is_some_and(|d| d.moving && d.tab == *id);
                    if *active || dragged {
                        let fill = if *active {
                            brand_blue().colorWithAlphaComponent(0.28)
                        } else {
                            NSColor::quaternaryLabelColor()
                        };
                        fill.setFill();
                        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(card, 7.0, 7.0).fill();
                    }
                    let dot = if *attention {
                        Some(NSColor::systemOrangeColor())
                    } else if *running {
                        Some(NSColor::systemGreenColor())
                    } else {
                        None
                    };
                    if let Some(color) = dot {
                        color.setFill();
                        let r = NSRect::new(NSPoint::new(17.0, top + 11.0), NSSize::new(7.0, 7.0));
                        NSBezierPath::bezierPathWithOvalInRect(r).fill();
                    }
                    let text_x = 32.0;
                    let text_w = width - text_x - 14.0;
                    draw_text(
                        title,
                        NSRect::new(NSPoint::new(text_x, top + 6.0), NSSize::new(text_w, 17.0)),
                        13.0,
                        false,
                        &NSColor::labelColor(),
                    );
                    draw_text(
                        subtitle,
                        NSRect::new(NSPoint::new(text_x, top + 23.0), NSSize::new(text_w, 15.0)),
                        11.0,
                        false,
                        &NSColor::secondaryLabelColor(),
                    );
                }
            }
        }
        if let Some(Drag {
            moving: true,
            drop: Some((_, _, y)),
            ..
        }) = drag
        {
            brand_blue().setFill();
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(12.0, y - 1.0),
                NSSize::new(width - 24.0, 2.0),
            ));
        }
    }
}

/// Index of the group row that owns the tabs ending before `end`.
fn shown_group_index(rows: &[Row], end: usize) -> usize {
    (0..end)
        .rev()
        .find(|&i| matches!(rows[i], Row::Group { .. }))
        .unwrap_or(0)
}

/// The cursor blue from the icon, for the active tab and drop line.
fn brand_blue() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        0x3B as f64 / 255.0,
        0x82 as f64 / 255.0,
        0xF6 as f64 / 255.0,
        1.0,
    )
}

fn row_height(row: &Row) -> f64 {
    match row {
        Row::Group { .. } => GROUP_H,
        Row::Tab { .. } => TAB_H,
    }
}

fn draw_text(text: &str, rect: NSRect, size: f64, bold: bool, color: &NSColor) {
    let font = if bold {
        NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightSemibold })
    } else {
        NSFont::systemFontOfSize(size)
    };
    let para = NSMutableParagraphStyle::new();
    para.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    let attrs: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
        NSDictionary::from_slices(
            &[
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
            ],
            &[&*font as &AnyObject, color as &AnyObject, &*para as &AnyObject],
        )
    };
    unsafe { NSString::from_str(text).drawInRect_withAttributes(rect, Some(&attrs)) };
}

/// The whole sidebar: background, search, + button and the scrolling list.
pub struct Sidebar {
    pub view: Retained<NSVisualEffectView>,
    list: Retained<SidebarList>,
    search: Retained<NSSearchField>,
    _targets: Vec<Retained<Target>>,
}

impl Sidebar {
    pub fn new(width: f64, height: f64, mtm: MainThreadMarker) -> Sidebar {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
        let view = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
        view.setMaterial(NSVisualEffectMaterial::Sidebar);
        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);

        let list = SidebarList::new(mtm);
        let scroll_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height - TOP_BAR));
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), scroll_frame);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        list.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, height - TOP_BAR),
        ));
        list.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        scroll.setDocumentView(Some(&list));
        view.addSubview(&scroll);

        let search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            NSRect::new(
                NSPoint::new(8.0, height - TOP_BAR + 8.0),
                NSSize::new(width - 46.0, 24.0),
            ),
        );
        search.setPlaceholderString(Some(&NSString::from_str("Search tabs")));
        search.setSendsSearchStringImmediately(true);
        search.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let weak_list = objc2::rc::Weak::from_retained(&list);
        let weak_search = objc2::rc::Weak::from_retained(&search);
        let on_search = Target::new(mtm, move |_| {
            if let (Some(list), Some(search)) = (weak_list.load(), weak_search.load()) {
                *list.ivars().query.borrow_mut() = search.stringValue().to_string();
                list.relayout();
            }
        });
        unsafe {
            search.setTarget(Some(&on_search));
            search.setAction(Some(Target::action()));
        }
        view.addSubview(&search);

        let plus = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            NSRect::new(
                NSPoint::new(width - 34.0, height - TOP_BAR + 8.0),
                NSSize::new(26.0, 24.0),
            ),
        );
        plus.setBordered(false);
        if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("plus"),
            Some(&NSString::from_str("New tab")),
        ) {
            plus.setImage(Some(&img));
        } else {
            plus.setTitle(&NSString::from_str("+"));
        }
        plus.setToolTip(Some(&NSString::from_str("New tab (⌘T)")));
        plus.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let weak_list = objc2::rc::Weak::from_retained(&list);
        let on_plus = Target::new(mtm, move |_| {
            if let Some(list) = weak_list.load() {
                list.emit(SidebarEvent::NewTab);
            }
        });
        unsafe {
            plus.setTarget(Some(&on_plus));
            plus.setAction(Some(Target::action()));
        }
        view.addSubview(&plus);

        Sidebar {
            view,
            list,
            search,
            _targets: vec![on_search, on_plus],
        }
    }

    pub fn set_handlers(
        &self,
        on_event: impl Fn(SidebarEvent) + 'static,
        menu_for: impl Fn(RowKey) -> Option<Retained<NSMenu>> + 'static,
    ) {
        *self.list.ivars().on_event.borrow_mut() = Some(Box::new(on_event));
        *self.list.ivars().menu_for.borrow_mut() = Some(Box::new(menu_for));
    }

    pub fn set_rows(&self, rows: Vec<Row>) {
        *self.list.ivars().rows.borrow_mut() = rows;
        self.list.relayout();
    }

    pub fn focus_search(&self) {
        if let Some(w) = self.view.window() {
            w.makeFirstResponder(Some(&self.search));
        }
    }
}
