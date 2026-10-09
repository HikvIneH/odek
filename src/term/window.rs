//! The workspace window: sidebar of grouped tabs on the left, the active
//! tab's split panes on the right. Owns the model (`Workspace`) and every
//! pane's terminal; inactive tabs keep running but aren't in the view tree.
//! One code viewer, created on first use, can sit in the active tab as a
//! pane; it moves to whichever tab asks for it and isn't saved.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSButton, NSColor, NSImage, NSMenu, NSMenuItem, NSModalResponse, NSRequestUserAttentionType, NSSplitView,
    NSSplitViewDividerStyle, NSTextField, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::header::PaneHeader;
use super::settings;
use super::sidebar::{Row, RowKey, Sidebar, SidebarEvent};
use super::target::Target;
use super::view::{TermView, ViewEvent};
use super::workspace::{Closed, Id, Node, Workspace};
use crate::app::{App, ViewerEvent};

const SIDEBAR_W: f64 = 240.0;
const HEADER_H: f64 = 24.0;
const SHELLS: &[&str] = &[
    "zsh", "bash", "fish", "sh", "dash", "tcsh", "csh", "ksh", "nu", "login",
];

struct Pane {
    /// Header (when the tab is split) above the terminal.
    container: Retained<NSView>,
    header: Retained<PaneHeader>,
    term: Retained<TermView>,
    title: String,
    cwd: Option<PathBuf>,
    /// Foreground program when it isn't a shell (e.g. "claude").
    program: Option<String>,
    attention: bool,
}

/// The code viewer pane: a header above the viewer's own views.
struct Viewer {
    id: Id,
    container: Retained<NSView>,
    header: Retained<PaneHeader>,
    app: Retained<App>,
}

/// Opened with the default app rather than the code viewer.
const NOT_TEXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "heic", "ico", "icns", "pdf", "mp4", "mov", "mp3", "wav", "zip",
    "gz", "tar", "dmg", "app", "ipa", "apk", "xlsx", "docx", "pptx", "key", "pages", "numbers", "sqlite",
    "db",
];

pub struct Workbench {
    mtm: MainThreadMarker,
    me: Weak<Workbench>,
    pub window: Retained<NSWindow>,
    /// Behind everything: blurs the desktop when the terminals are translucent.
    blur: Retained<NSVisualEffectView>,
    split: Retained<NSSplitView>,
    sidebar: Sidebar,
    content: Retained<NSView>,
    ws: RefCell<Workspace>,
    panes: RefCell<HashMap<Id, Pane>>,
    /// Targets for the pane close buttons and context menus (held weakly by AppKit).
    targets: RefCell<Vec<Retained<Target>>>,
    menu_targets: RefCell<Vec<Retained<Target>>>,
    sidebar_hidden: Cell<bool>,
    last_saved: RefCell<String>,
    viewer: RefCell<Option<Viewer>>,
}

impl Workbench {
    pub fn new(delegate: &ProtocolObject<dyn NSWindowDelegate>, mtm: MainThreadMarker) -> Rc<Workbench> {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1200.0, 760.0));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTabbingMode(objc2_app_kit::NSWindowTabbingMode::Disallowed);
        window.setContentMinSize(NSSize::new(480.0, 240.0));
        window.setDelegate(Some(delegate));
        window.center();
        window.setFrameAutosaveName(&NSString::from_str("OdekWorkspace"));

        let bounds = window.contentView().map_or(frame, |v| v.bounds());
        let split = NSSplitView::initWithFrame(NSSplitView::alloc(mtm), bounds);
        split.setVertical(true);
        split.setDividerStyle(NSSplitViewDividerStyle::Thin);
        split.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let sidebar = Sidebar::new(SIDEBAR_W, bounds.size.height, mtm);
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(bounds.size.width - SIDEBAR_W, bounds.size.height),
            ),
        );
        content.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        split.addSubview(&sidebar.view);
        split.addSubview(&content);
        let blur = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), bounds);
        // HUD is a dark, see-through frosted glass; the window-background
        // materials are nearly opaque and would hide the desktop.
        blur.setMaterial(NSVisualEffectMaterial::HUDWindow);
        blur.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        blur.setState(NSVisualEffectState::Active);
        blur.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        // The blur sits behind the panes as a sibling, so hiding it (opaque
        // windows) never hides them.
        let root = NSView::initWithFrame(NSView::alloc(mtm), bounds);
        root.addSubview(&blur);
        root.addSubview(&split);
        window.setContentView(Some(&root));
        split.adjustSubviews();
        split.setPosition_ofDividerAtIndex(SIDEBAR_W, 0);
        split.setHoldingPriority_forSubviewAtIndex(260.0, 0);

        let bench = Rc::new_cyclic(|me| Workbench {
            mtm,
            me: me.clone(),
            window,
            blur,
            split,
            sidebar,
            content,
            ws: RefCell::new(Workspace::default()),
            panes: RefCell::new(HashMap::new()),
            targets: RefCell::new(Vec::new()),
            menu_targets: RefCell::new(Vec::new()),
            sidebar_hidden: Cell::new(false),
            last_saved: RefCell::new(String::new()),
            viewer: RefCell::new(None),
        });
        bench.apply_appearance();
        let (w1, w2) = (bench.me.clone(), bench.me.clone());
        bench.sidebar.set_handlers(
            move |e| {
                if let Some(b) = w1.upgrade() {
                    b.sidebar_event(e);
                }
            },
            move |key| w2.upgrade().and_then(|b| b.context_menu(key)),
        );
        bench
    }

    // ---- startup and saving ----

    fn save_path() -> Option<PathBuf> {
        // Tests point this elsewhere so they never touch the real workspace.
        if let Some(p) = std::env::var_os("ODEK_WORKSPACE_FILE") {
            return Some(PathBuf::from(p));
        }
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Application Support/Odek/workspace.txt"))
    }

    /// Restore the saved workspace (or start one), plus a tab for `open` if given.
    pub fn start(&self, open: Option<PathBuf>, show: bool) {
        let saved = Self::save_path().and_then(|p| std::fs::read_to_string(p).ok());
        let restored = saved.as_deref().and_then(Workspace::load);
        match restored {
            Some((ws, panes)) if !panes.is_empty() => {
                *self.ws.borrow_mut() = ws;
                for (id, dir) in panes {
                    self.make_pane(id, &dir);
                }
            }
            _ => {
                let mut ws = Workspace::default();
                ws.add_group("Main");
                *self.ws.borrow_mut() = ws;
                if open.is_none() {
                    let home = std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| "/".into());
                    self.new_tab_in(&home);
                }
            }
        }
        let file = open.as_ref().filter(|p| p.is_file()).cloned();
        if let Some(dir) = open.filter(|p| !p.is_file()) {
            self.new_tab_in(&dir);
        }
        self.show_active();
        if show {
            self.window.makeKeyAndOrderFront(None);
        }
        self.focus_active_pane();
        if let Some(file) = file {
            self.open_in_viewer(&file, None, None);
        }
    }

    /// A folder or file handed to the running app (`odek <path>`, Finder,
    /// the Dock): a new tab, or the file in the viewer.
    pub fn open_path(&self, path: &Path) {
        if path.is_dir() {
            self.new_tab_in(path);
            self.show_active();
            self.focus_active_pane();
            self.save();
        } else {
            self.open_in_viewer(path, None, None);
        }
        self.window.makeKeyAndOrderFront(None);
    }

    #[cfg_attr(not(feature = "selftest"), allow(dead_code))]
    pub fn focused_term(&self) -> Option<Retained<TermView>> {
        let id = self.focused_pane()?;
        self.panes.borrow().get(&id).map(|p| p.term.clone())
    }

    #[cfg(feature = "selftest")]
    pub fn name_active_tab(&self, name: &str) {
        let active = self.ws.borrow().active;
        if let Some(a) = active
            && let Some(t) = self.ws.borrow_mut().tab_mut(a)
        {
            t.name = Some(name.to_string());
        }
        self.refresh();
    }

    #[cfg(feature = "selftest")]
    pub fn name_active_group(&self, name: &str) {
        let group = self.ws.borrow().active.and_then(|t| self.ws.borrow().group_of(t));
        if let Some(g) = group
            && let Some(g) = self.ws.borrow_mut().groups.iter_mut().find(|x| x.id == g)
        {
            g.name = name.to_string();
        }
        self.refresh();
    }

    #[cfg(feature = "selftest")]
    pub fn move_active_to_new_group(&self, name: &str) {
        let Some(tab) = self.ws.borrow().active else {
            return;
        };
        let group = self.ws.borrow_mut().add_group(name);
        self.ws.borrow_mut().move_tab(tab, group, 0);
        self.refresh();
    }

    pub fn save(&self) {
        self.sync_ratios();
        for p in self.panes.borrow_mut().values_mut() {
            if let Some(cwd) = p.term.session_cwd() {
                p.cwd = Some(cwd);
            }
        }
        let text = {
            let panes = self.panes.borrow();
            let mut ws = self.ws.borrow().clone();
            if let Some(v) = self.viewer_id() {
                ws.close_pane(v);
            }
            ws.save(|id| panes.get(&id).and_then(|p| p.cwd.clone()))
        };
        if *self.last_saved.borrow() == text {
            return;
        }
        if let Some(path) = Self::save_path() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // Write then rename, so a crash never leaves half a file.
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, &text).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                *self.last_saved.borrow_mut() = text;
            }
        }
    }

    // ---- panes ----

    fn make_pane(&self, id: Id, dir: &Path) {
        let mtm = self.mtm;
        let size = self.content.bounds().size;
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let container = NSView::initWithFrame(NSView::alloc(mtm), frame);
        container.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        let header = PaneHeader::new(
            NSRect::new(
                NSPoint::new(0.0, size.height - HEADER_H),
                NSSize::new(size.width, HEADER_H),
            ),
            mtm,
        );
        header.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        header.addSubview(&self.close_button(id, size.width));
        container.addSubview(&header);

        let term = TermView::new(frame, mtm);
        term.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        container.addSubview(&term);
        let me = self.me.clone();
        term.set_on_event(move |e| {
            if let Some(b) = me.upgrade() {
                b.pane_event(id, e);
            }
        });
        let dir = if dir.is_dir() {
            dir.to_path_buf()
        } else {
            std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
        };
        if let Err(e) = term.start(&dir, None) {
            eprintln!("odek: could not start shell: {e}");
        }
        self.panes.borrow_mut().insert(
            id,
            Pane {
                container,
                header,
                term,
                title: String::new(),
                cwd: Some(dir),
                program: None,
                attention: false,
            },
        );
    }

    /// The × at the right of a pane header.
    fn close_button(&self, id: Id, width: f64) -> Retained<NSButton> {
        let close = NSButton::initWithFrame(
            NSButton::alloc(self.mtm),
            NSRect::new(NSPoint::new(width - 26.0, 2.0), NSSize::new(20.0, 20.0)),
        );
        close.setBordered(false);
        if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("xmark"),
            Some(&NSString::from_str("Close pane")),
        ) {
            close.setImage(Some(&img));
        }
        close.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        let me = self.me.clone();
        let target = Target::new(self.mtm, move |_| {
            if let Some(b) = me.upgrade() {
                b.request_close_pane(id);
            }
        });
        unsafe {
            close.setTarget(Some(&target));
            close.setAction(Some(Target::action()));
        }
        self.targets.borrow_mut().push(target);
        close
    }

    fn pane_event(&self, id: Id, e: ViewEvent) {
        match e {
            ViewEvent::Title(t) => {
                if let Some(p) = self.panes.borrow_mut().get_mut(&id) {
                    p.title = t;
                }
                self.refresh();
            }
            ViewEvent::Attention(msg) => {
                let seen = self.window.isKeyWindow() && self.focused_pane() == Some(id);
                if !seen {
                    if let Some(p) = self.panes.borrow_mut().get_mut(&id) {
                        p.attention = true;
                    }
                    let app = NSApplication::sharedApplication(self.mtm);
                    let notified = self.notify_attention(id, msg.as_deref(), app.isActive());
                    if !app.isActive() && !notified {
                        app.requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
                    }
                    self.refresh();
                }
            }
            ViewEvent::Exited(_) => self.close_pane(id),
            ViewEvent::OpenPath { path, line, col } => {
                let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
                if ext.is_some_and(|e| NOT_TEXT.contains(&e.as_str())) {
                    if let Some(p) = path.to_str() {
                        let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(p));
                        objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url);
                    }
                } else {
                    self.open_in_viewer(&path, line, col);
                }
            }
            ViewEvent::Focused => {
                let tab = self.ws.borrow().tab_of_pane(id);
                if let Some(tab) = tab {
                    if let Some(t) = self.ws.borrow_mut().tab_mut(tab) {
                        t.focus = id;
                    }
                    if let Some(p) = self.panes.borrow_mut().get_mut(&id) {
                        p.attention = false;
                    }
                    self.refresh();
                }
            }
        }
    }

    /// Post a desktop notification when the pane's tab isn't in front.
    fn notify_attention(&self, id: Id, msg: Option<&str>, app_active: bool) -> bool {
        let (tab, tab_active) = {
            let ws = self.ws.borrow();
            let Some(tab) = ws.tab_of_pane(id) else {
                return false;
            };
            (tab, ws.active == Some(tab))
        };
        if !super::notify::should_notify(app_active, self.window.isKeyWindow(), tab_active) {
            return false;
        }
        let (title, dir) = {
            let ws = self.ws.borrow();
            ws.tab(tab).map(|t| self.tab_title(t)).unwrap_or_default()
        };
        super::notify::post(
            tab,
            id,
            &super::notify::title_for(&title, &dir),
            &super::notify::body_for(msg),
        )
    }

    /// A notification was clicked: show that tab and focus the pane.
    pub fn reveal_pane(&self, id: Id) {
        self.window.makeKeyAndOrderFront(None);
        let Some(tab) = self.ws.borrow().tab_of_pane(id) else {
            return;
        };
        if let Some(t) = self.ws.borrow_mut().tab_mut(tab) {
            t.focus = id;
        }
        self.select_tab(tab);
    }

    fn focused_pane(&self) -> Option<Id> {
        self.ws.borrow().active_tab().map(|t| t.focus)
    }

    fn focus_active_pane(&self) {
        let Some(id) = self.focused_pane() else { return };
        if Some(id) == self.viewer_id() {
            let app = self.viewer.borrow().as_ref().map(|v| v.app.clone());
            if let Some(app) = app {
                app.focus();
            }
            self.refresh();
            return;
        }
        // Clone first: becoming first responder reports back into `pane_event`.
        let term = self.panes.borrow().get(&id).map(|p| p.term.clone());
        if let Some(term) = term {
            self.window.makeFirstResponder(Some(&term));
        }
        if let Some(p) = self.panes.borrow_mut().get_mut(&id) {
            p.attention = false;
        }
        if let Some(tab) = self.ws.borrow().tab_of_pane(id) {
            super::notify::clear(tab);
        }
        self.refresh();
    }

    /// The folder new tabs and splits start in: the focused pane's.
    fn current_dir(&self) -> PathBuf {
        let focus = self.focused_pane();
        if focus.is_some() && focus == self.viewer_id() {
            let root = self.viewer.borrow().as_ref().and_then(|v| v.app.root_dir());
            if let Some(root) = root {
                return root;
            }
        }
        focus
            .and_then(|id| {
                self.panes
                    .borrow()
                    .get(&id)
                    .and_then(|p| p.term.session_cwd().or(p.cwd.clone()))
            })
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| "/".into())
    }

    // ---- layout ----

    /// Put the active tab's split tree into the content area.
    fn show_active(&self) {
        for v in self.content.subviews().iter() {
            v.removeFromSuperview();
        }
        let tab = self.ws.borrow().active_tab().cloned();
        let Some(tab) = tab else {
            self.refresh();
            return;
        };
        let split = matches!(tab.root, Node::Split { .. });
        let bounds = self.content.bounds();
        let view = self.build(&tab.root, bounds, split);
        view.setFrame(bounds);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        self.content.addSubview(&view);
        place_dividers(&tab.root, &view);
        self.refresh();
    }

    fn build(&self, node: &Node, frame: NSRect, headers: bool) -> Retained<NSView> {
        match node {
            Node::Pane(id) if Some(*id) == self.viewer_id() => {
                let viewer = self.viewer.borrow();
                let v = viewer.as_ref().unwrap();
                v.container.setFrame(frame);
                v.header.setHidden(!headers);
                let h = frame.size.height - if headers { HEADER_H } else { 0.0 };
                v.app.root_view().setFrame(NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(frame.size.width, h.max(10.0)),
                ));
                v.container.clone()
            }
            Node::Pane(id) => {
                let panes = self.panes.borrow();
                let p = &panes[id];
                p.container.setFrame(frame);
                p.header.setHidden(!headers);
                let h = frame.size.height - if headers { HEADER_H } else { 0.0 };
                p.term.setFrame(NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(frame.size.width, h.max(10.0)),
                ));
                p.container.clone()
            }
            Node::Split {
                across,
                first,
                second,
                ..
            } => {
                let split = NSSplitView::initWithFrame(NSSplitView::alloc(self.mtm), frame);
                split.setVertical(*across);
                split.setDividerStyle(NSSplitViewDividerStyle::Thin);
                let a = self.build(first, frame, headers);
                let b = self.build(second, frame, headers);
                split.addSubview(&a);
                split.addSubview(&b);
                split.adjustSubviews();
                Retained::into_super(split)
            }
        }
    }

    /// Read divider positions back into the model before saving or rebuilding.
    fn sync_ratios(&self) {
        let Some(top) = self.content.subviews().firstObject() else {
            return;
        };
        let mut ws = self.ws.borrow_mut();
        let Some(active) = ws.active else { return };
        if let Some(tab) = ws.tab_mut(active) {
            read_ratios(&mut tab.root, &top);
        }
    }

    // ---- sidebar ----

    fn tab_title(&self, tab: &super::workspace::Tab) -> (String, String) {
        if let Some(v) = self.viewer.borrow().as_ref()
            && v.id == tab.focus
        {
            let dir = v.app.root_dir().as_deref().map(tilde).unwrap_or_default();
            return (tab.name.clone().unwrap_or_else(|| v.app.display_title()), dir);
        }
        let panes = self.panes.borrow();
        let p = panes.get(&tab.focus);
        let dir = p.and_then(|p| p.cwd.as_deref()).map(tilde).unwrap_or_default();
        let auto = p.map(pane_title).unwrap_or_default();
        (tab.name.clone().unwrap_or(auto), dir)
    }

    /// Push the model to the sidebar, pane headers and window title.
    fn refresh(&self) {
        let ws = self.ws.borrow();
        let mut rows = Vec::new();
        for g in &ws.groups {
            rows.push(Row::Group {
                id: g.id,
                name: g.name.clone(),
                collapsed: g.collapsed,
                count: g.tabs.len(),
            });
            for t in &g.tabs {
                let (title, subtitle) = self.tab_title(t);
                let panes = self.panes.borrow();
                let ids = t.root.panes();
                let attention = ids.iter().any(|i| panes.get(i).is_some_and(|p| p.attention));
                let running = ids
                    .iter()
                    .any(|i| panes.get(i).is_some_and(|p| p.program.is_some()));
                rows.push(Row::Tab {
                    id: t.id,
                    title,
                    subtitle,
                    active: ws.active == Some(t.id),
                    attention,
                    running,
                });
            }
        }
        self.sidebar.set_rows(rows);
        if let Some(tab) = ws.active_tab() {
            let (title, _) = self.tab_title(tab);
            self.window.setTitle(&NSString::from_str(&title));
            let panes = self.panes.borrow();
            for id in tab.root.panes() {
                if let Some(p) = panes.get(&id) {
                    p.header.set(&pane_title(p), id == tab.focus);
                }
            }
            if let Some(v) = self.viewer.borrow().as_ref() {
                let title = v.app.display_title();
                v.header
                    .set(if title.is_empty() { "Files" } else { &title }, v.id == tab.focus);
            }
        } else {
            self.window.setTitle(&NSString::from_str("Odek"));
        }
    }

    fn sidebar_event(&self, e: SidebarEvent) {
        match e {
            SidebarEvent::Select(tab) => self.select_tab(tab),
            SidebarEvent::ToggleGroup(g) => {
                if let Some(g) = self.ws.borrow_mut().groups.iter_mut().find(|x| x.id == g) {
                    g.collapsed = !g.collapsed;
                }
                self.refresh();
                self.save();
            }
            SidebarEvent::RenameTab(tab) => self.rename_tab(tab),
            SidebarEvent::RenameGroup(g) => self.rename_group(g),
            SidebarEvent::Move { tab, group, index } => {
                self.ws.borrow_mut().move_tab(tab, group, index);
                self.refresh();
                self.save();
            }
            SidebarEvent::NewTab => self.new_tab(),
        }
    }

    fn context_menu(&self, key: RowKey) -> Option<Retained<NSMenu>> {
        let menu = NSMenu::new(self.mtm);
        self.menu_targets.borrow_mut().clear();
        let groups: Vec<(Id, String)> = self
            .ws
            .borrow()
            .groups
            .iter()
            .map(|g| (g.id, g.name.clone()))
            .collect();
        match key {
            RowKey::Tab(tab) => {
                self.add_item(&menu, "Rename Tab…", move |b| b.rename_tab(tab));
                let current = self.ws.borrow().group_of(tab);
                let sub = NSMenu::new(self.mtm);
                for (gid, name) in groups.iter().filter(|(g, _)| Some(*g) != current) {
                    let gid = *gid;
                    self.add_item(&sub, name, move |b| {
                        b.ws.borrow_mut().move_tab(tab, gid, usize::MAX);
                        b.refresh();
                        b.save();
                    });
                }
                if !groups.is_empty() {
                    sub.addItem(&NSMenuItem::separatorItem(self.mtm));
                }
                self.add_item(&sub, "New Group…", move |b| b.new_group_with(Some(tab)));
                let holder = NSMenuItem::new(self.mtm);
                holder.setTitle(&NSString::from_str("Move to Group"));
                holder.setSubmenu(Some(&sub));
                menu.addItem(&holder);
                menu.addItem(&NSMenuItem::separatorItem(self.mtm));
                self.add_item(&menu, "Close Tab", move |b| b.request_close_tab(tab));
            }
            RowKey::Group(g) => {
                self.add_item(&menu, "New Tab in Group", move |b| {
                    let dir = b.current_dir();
                    b.new_tab_at(g, None, &dir);
                });
                self.add_item(&menu, "Rename Group…", move |b| b.rename_group(g));
                self.add_item(&menu, "New Group…", |b| b.new_group_with(None));
                if groups.len() > 1 {
                    menu.addItem(&NSMenuItem::separatorItem(self.mtm));
                    self.add_item(&menu, "Delete Group (keep its tabs)", move |b| {
                        b.ws.borrow_mut().remove_group(g);
                        b.refresh();
                        b.save();
                    });
                }
            }
            RowKey::Empty => {
                self.add_item(&menu, "New Tab", |b| b.new_tab());
                self.add_item(&menu, "New Group…", |b| b.new_group_with(None));
            }
        }
        Some(menu)
    }

    fn add_item(&self, menu: &NSMenu, title: &str, action: impl Fn(&Workbench) + 'static) {
        let me = self.me.clone();
        let target = Target::new(self.mtm, move |_| {
            if let Some(b) = me.upgrade() {
                action(&b);
            }
        });
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str(title),
                Some(Target::action()),
                &NSString::from_str(""),
            )
        };
        unsafe { item.setTarget(Some(&target)) };
        menu.addItem(&item);
        self.menu_targets.borrow_mut().push(target);
    }

    // ---- actions (menus and sidebar) ----

    pub fn select_tab(&self, tab: Id) {
        if self.ws.borrow().active == Some(tab) {
            self.focus_active_pane();
            return;
        }
        self.sync_ratios();
        self.ws.borrow_mut().active = Some(tab);
        self.show_active();
        self.focus_active_pane();
        self.save();
    }

    pub fn new_tab(&self) {
        let dir = self.current_dir();
        self.new_tab_in(&dir);
        self.show_active();
        self.focus_active_pane();
        self.save();
    }

    fn new_tab_in(&self, dir: &Path) {
        let (group, after) = {
            let ws = self.ws.borrow();
            let after = ws.active;
            let group = after
                .and_then(|t| ws.group_of(t))
                .or(ws.groups.first().map(|g| g.id));
            (group, after)
        };
        let group = group.unwrap_or_else(|| self.ws.borrow_mut().add_group("Main"));
        self.new_tab_at(group, after, dir);
    }

    fn new_tab_at(&self, group: Id, after: Option<Id>, dir: &Path) {
        self.sync_ratios();
        let pane = self.ws.borrow_mut().new_id();
        self.make_pane(pane, dir);
        self.ws.borrow_mut().add_tab(group, after, pane);
        self.show_active();
        self.focus_active_pane();
        self.save();
    }

    pub fn split(&self, across: bool) {
        let Some(tab) = self.ws.borrow().active else {
            return;
        };
        self.sync_ratios();
        let dir = self.current_dir();
        let pane = self.ws.borrow_mut().new_id();
        self.make_pane(pane, &dir);
        self.ws.borrow_mut().split(tab, pane, across);
        self.show_active();
        self.focus_active_pane();
        self.save();
    }

    /// Move focus to the next or previous pane of the active tab.
    pub fn cycle_pane(&self, forward: bool) {
        {
            let mut ws = self.ws.borrow_mut();
            let Some(active) = ws.active else { return };
            let Some(tab) = ws.tab_mut(active) else { return };
            let ids = tab.root.panes();
            let n = ids.len();
            let i = ids.iter().position(|&p| p == tab.focus).unwrap_or(0);
            tab.focus = ids[if forward { (i + 1) % n } else { (i + n - 1) % n }];
        }
        self.focus_active_pane();
    }

    pub fn cycle_tab(&self, forward: bool) {
        let next = self.ws.borrow().cycle(forward);
        if let Some(t) = next {
            self.select_tab(t);
        }
    }

    /// ⌘1…⌘9: the n-th tab in sidebar order (⌘9 = last).
    pub fn select_index(&self, n: usize) {
        let tabs = self.ws.borrow().tabs();
        let pick = if n >= 9 { tabs.last() } else { tabs.get(n - 1) };
        if let Some(&t) = pick {
            self.select_tab(t);
        }
    }

    pub fn toggle_sidebar(&self) {
        let hidden = !self.sidebar_hidden.get();
        self.sidebar_hidden.set(hidden);
        self.sidebar.view.setHidden(hidden);
        self.split.adjustSubviews();
        if !hidden {
            self.split.setPosition_ofDividerAtIndex(SIDEBAR_W, 0);
        }
    }

    pub fn search_tabs(&self) {
        if self.sidebar_hidden.get() {
            self.toggle_sidebar();
        }
        self.sidebar.focus_search();
    }

    /// Programs still running in these panes (not counting idle shells).
    fn running_in(&self, ids: &[Id]) -> Vec<String> {
        let panes = self.panes.borrow();
        ids.iter()
            .filter_map(|id| panes.get(id).and_then(|p| program_of(&p.term)))
            .collect()
    }

    pub fn request_close_focused(&self) {
        if let Some(id) = self.focused_pane() {
            self.request_close_pane(id);
        }
    }

    pub fn request_close_pane(&self, id: Id) {
        if Some(id) == self.viewer_id() {
            if self.viewer_confirm_discard() {
                self.close_pane(id);
            }
            return;
        }
        let running = self.running_in(&[id]);
        if running.is_empty() {
            return self.close_pane(id);
        }
        let me = self.me.clone();
        self.confirm(
            &format!("Close this pane? {} is still running.", running.join(", ")),
            "Its process will be ended.",
            move || {
                if let Some(b) = me.upgrade() {
                    b.close_pane(id);
                }
            },
        );
    }

    pub fn request_close_active_tab(&self) {
        let active = self.ws.borrow().active;
        if let Some(t) = active {
            self.request_close_tab(t);
        }
    }

    fn request_close_tab(&self, tab: Id) {
        let ids = self
            .ws
            .borrow()
            .tab(tab)
            .map(|t| t.root.panes())
            .unwrap_or_default();
        if self.viewer_id().is_some_and(|v| ids.contains(&v)) && !self.viewer_confirm_discard() {
            return;
        }
        let running = self.running_in(&ids);
        let me = self.me.clone();
        let close = move || {
            if let Some(b) = me.upgrade() {
                for id in &ids {
                    b.close_pane(*id);
                }
            }
        };
        if running.is_empty() {
            close();
        } else {
            self.confirm(
                &format!("Close this tab? {} still running.", list_running(&running)),
                "Their processes will be ended.",
                close,
            );
        }
    }

    fn close_pane(&self, id: Id) {
        self.sync_ratios();
        let closed = self.ws.borrow_mut().close_pane(id);
        if let Some(p) = self.panes.borrow_mut().remove(&id) {
            p.term.shutdown();
            p.container.removeFromSuperview();
        }
        if Some(id) == self.viewer_id() {
            let viewer = self.viewer.borrow();
            let v = viewer.as_ref().unwrap();
            // Free the open files; the viewer itself stays for next time.
            v.app.release_files();
            v.container.removeFromSuperview();
        }
        if closed == Closed::Nothing {
            return;
        }
        if self.ws.borrow().tabs().is_empty() {
            // Last tab gone: close the window (and with it, the app).
            self.save();
            self.window.close();
            return;
        }
        self.show_active();
        self.focus_active_pane();
        self.save();
    }

    /// Names of programs running anywhere, for the quit confirmation.
    pub fn running_everywhere(&self) -> Vec<String> {
        let ids: Vec<Id> = self.panes.borrow().keys().copied().collect();
        self.running_in(&ids)
    }

    pub fn shutdown_all(&self) {
        self.save();
        for (_, p) in self.panes.borrow_mut().drain() {
            p.term.shutdown();
        }
    }

    pub fn new_group(&self) {
        self.new_group_with(None);
    }

    /// Ask for a name, create the group, and move `tab` (or a new tab) into it.
    fn new_group_with(&self, tab: Option<Id>) {
        let me = self.me.clone();
        self.ask_name("New group", "", move |name| {
            let Some(b) = me.upgrade() else { return };
            let group = b.ws.borrow_mut().add_group(&name);
            match tab {
                Some(t) => {
                    b.ws.borrow_mut().move_tab(t, group, 0);
                    b.refresh();
                    b.save();
                }
                None => {
                    let dir = b.current_dir();
                    b.new_tab_at(group, None, &dir);
                }
            }
        });
    }

    pub fn rename_active_tab(&self) {
        let active = self.ws.borrow().active;
        if let Some(t) = active {
            self.rename_tab(t);
        }
    }

    fn rename_tab(&self, tab: Id) {
        let current = self
            .ws
            .borrow()
            .tab(tab)
            .and_then(|t| t.name.clone())
            .unwrap_or_default();
        let me = self.me.clone();
        self.ask_name(
            "Rename tab (empty: follow the program's title)",
            &current,
            move |name| {
                let Some(b) = me.upgrade() else { return };
                if let Some(t) = b.ws.borrow_mut().tab_mut(tab) {
                    t.name = (!name.is_empty()).then_some(name);
                }
                b.refresh();
                b.save();
            },
        );
    }

    fn rename_group(&self, group: Id) {
        let current = self
            .ws
            .borrow()
            .groups
            .iter()
            .find(|g| g.id == group)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let me = self.me.clone();
        self.ask_name("Rename group", &current, move |name| {
            let Some(b) = me.upgrade() else { return };
            if name.is_empty() {
                return;
            }
            if let Some(g) = b.ws.borrow_mut().groups.iter_mut().find(|g| g.id == group) {
                g.name = name;
            }
            b.refresh();
            b.save();
        });
    }

    /// Opacity and blur from settings: a translucent window shows the
    /// desktop (blurred, or not) through the terminals.
    pub fn apply_appearance(&self) {
        let opacity = settings::opacity();
        let clear = opacity < 1.0;
        self.window.setOpaque(!clear);
        let bg = if clear {
            NSColor::clearColor()
        } else {
            NSColor::windowBackgroundColor()
        };
        self.window.setBackgroundColor(Some(&bg));
        self.blur.setHidden(!(clear && settings::blur()));
    }

    // ---- code viewer ----

    fn viewer_id(&self) -> Option<Id> {
        self.viewer.borrow().as_ref().map(|v| v.id)
    }

    /// True when the viewer has no unsaved files, or the user dealt with them.
    pub fn viewer_confirm_discard(&self) -> bool {
        let app = self.viewer.borrow().as_ref().map(|v| v.app.clone());
        app.is_none_or(|a| a.confirm_discard_all())
    }

    pub fn app_activated(&self) {
        let app = self.viewer.borrow().as_ref().map(|v| v.app.clone());
        if let Some(app) = app {
            app.app_activated();
        }
    }

    fn ensure_viewer(&self) -> Retained<App> {
        if let Some(v) = self.viewer.borrow().as_ref() {
            return v.app.clone();
        }
        let mtm = self.mtm;
        let id = self.ws.borrow_mut().new_id();
        let size = self.content.bounds().size;
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let container = NSView::initWithFrame(NSView::alloc(mtm), frame);
        container.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let header = PaneHeader::new(
            NSRect::new(
                NSPoint::new(0.0, size.height - HEADER_H),
                NSSize::new(size.width, HEADER_H),
            ),
            mtm,
        );
        header.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        header.addSubview(&self.close_button(id, size.width));
        container.addSubview(&header);
        let app = App::new_embedded(
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(size.width, size.height - HEADER_H),
            ),
            mtm,
        );
        container.addSubview(&app.root_view());
        let me = self.me.clone();
        app.set_on_event(move |e| {
            if let Some(b) = me.upgrade() {
                b.viewer_event(id, e);
            }
        });
        *self.viewer.borrow_mut() = Some(Viewer {
            id,
            container,
            header,
            app: app.clone(),
        });
        app
    }

    fn viewer_event(&self, id: Id, e: ViewerEvent) {
        match e {
            ViewerEvent::Title => self.refresh(),
            ViewerEvent::Focused => {
                let tab = self.ws.borrow().tab_of_pane(id);
                if let Some(tab) = tab
                    && let Some(t) = self.ws.borrow_mut().tab_mut(tab)
                {
                    t.focus = id;
                }
                self.refresh();
            }
            ViewerEvent::Close => self.request_close_pane(id),
        }
    }

    /// Put the viewer in the active tab, beside the focused pane, taking it
    /// out of whatever tab had it.
    fn place_viewer(&self) -> Retained<App> {
        let app = self.ensure_viewer();
        let id = self.viewer_id().unwrap();
        let Some(active) = self.ws.borrow().active else {
            return app;
        };
        let here = self.ws.borrow().tab_of_pane(id);
        if here == Some(active) {
            if let Some(t) = self.ws.borrow_mut().tab_mut(active) {
                t.focus = id;
            }
            return app;
        }
        self.sync_ratios();
        if let Some(other) = here {
            let mut ws = self.ws.borrow_mut();
            let rest = ws.tab(other).and_then(|t| t.root.clone().without(id));
            match rest {
                Some(root) => {
                    let t = ws.tab_mut(other).unwrap();
                    if t.focus == id {
                        t.focus = root.panes()[0];
                    }
                    t.root = root;
                }
                None => ws.remove_tab(other),
            }
            ws.active = Some(active);
        }
        {
            let mut ws = self.ws.borrow_mut();
            ws.split(active, id, true);
            // Code wants a little more width than the terminal beside it.
            if let Some(t) = ws.tab_mut(active) {
                t.root.set_ratio_before(id, 0.45);
            }
        }
        self.show_active();
        app
    }

    /// Open a file (at a line) or folder from a terminal in the viewer.
    pub fn open_in_viewer(&self, path: &Path, line: Option<u32>, col: Option<u32>) {
        let near = self.current_dir();
        let app = self.place_viewer();
        app.open_location(path, line, col, Some(&near));
        app.focus();
        self.refresh();
    }

    /// ⌘P: quick open in the focused pane's project.
    pub fn quick_open_here(&self) {
        let dir = self.current_dir();
        let app = self.place_viewer();
        app.quick_open_in(&dir);
        self.refresh();
    }

    /// ⇧⌘E: the file tree for the focused pane's folder.
    pub fn show_files_here(&self) {
        let dir = self.current_dir();
        let app = self.place_viewer();
        app.show_tree_in(&dir);
        self.refresh();
    }

    // ---- periodic ----

    /// Every second: folders and running programs for the sidebar; save when changed.
    pub fn tick(&self) {
        let mut changed = false;
        for p in self.panes.borrow_mut().values_mut() {
            let cwd = p.term.session_cwd();
            if cwd.is_some() && cwd != p.cwd {
                p.cwd = cwd;
                changed = true;
            }
            let program = program_of(&p.term);
            if program != p.program {
                p.program = program;
                changed = true;
            }
        }
        if changed {
            self.refresh();
            self.save();
        }
    }

    pub fn window_became_key(&self) {
        if let Some(id) = self.focused_pane()
            && let Some(p) = self.panes.borrow_mut().get_mut(&id)
            && p.attention
        {
            p.attention = false;
        } else {
            return;
        }
        self.refresh();
    }

    // ---- dialogs ----

    fn confirm(&self, message: &str, info: &str, on_yes: impl Fn() + 'static) {
        let alert = NSAlert::new(self.mtm);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(info));
        alert.addButtonWithTitle(&NSString::from_str("Close"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let done = RcBlock::new(move |resp: NSModalResponse| {
            if resp == NSAlertFirstButtonReturn {
                on_yes();
            }
        });
        alert.beginSheetModalForWindow_completionHandler(&self.window, Some(&done));
    }

    fn ask_name(&self, message: &str, current: &str, on_ok: impl Fn(String) + 'static) {
        let alert = NSAlert::new(self.mtm);
        alert.setMessageText(&NSString::from_str(message));
        alert.addButtonWithTitle(&NSString::from_str("OK"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let field = NSTextField::initWithFrame(
            NSTextField::alloc(self.mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 24.0)),
        );
        field.setStringValue(&NSString::from_str(current));
        alert.setAccessoryView(Some(&field));
        alert.window().setInitialFirstResponder(Some(&field));
        let f = field.clone();
        let done = RcBlock::new(move |resp: NSModalResponse| {
            if resp == NSAlertFirstButtonReturn {
                on_ok(f.stringValue().to_string().trim().to_string());
            }
        });
        alert.beginSheetModalForWindow_completionHandler(&self.window, Some(&done));
    }
}

/// What a pane is called: while a program runs, its own title (Claude Code
/// puts its status and task there); at a shell prompt, the folder.
fn pane_title(p: &Pane) -> String {
    let dir = p.cwd.as_deref().map(tilde).unwrap_or_default();
    match &p.program {
        Some(prog) if p.title.is_empty() => prog.clone(),
        Some(_) => p.title.clone(),
        None => dir,
    }
}

fn place_dividers(node: &Node, view: &NSView) {
    if let Node::Split {
        across,
        ratio,
        first,
        second,
    } = node
        && let Some(split) = view.downcast_ref::<NSSplitView>()
    {
        let size = split.bounds().size;
        let total = if *across { size.width } else { size.height } - split.dividerThickness();
        split.setPosition_ofDividerAtIndex((total * ratio).round(), 0);
        let subs = split.subviews();
        if subs.len() == 2 {
            place_dividers(first, &subs.objectAtIndex(0));
            place_dividers(second, &subs.objectAtIndex(1));
        }
    }
}

fn read_ratios(node: &mut Node, view: &NSView) {
    if let Node::Split {
        across,
        ratio,
        first,
        second,
    } = node
        && let Some(split) = view.downcast_ref::<NSSplitView>()
    {
        let subs = split.subviews();
        if subs.len() != 2 {
            return;
        }
        let a = subs.objectAtIndex(0).frame().size;
        let size = split.bounds().size;
        let total = if *across { size.width } else { size.height } - split.dividerThickness();
        if total > 0.0 {
            *ratio = ((if *across { a.width } else { a.height }) / total).clamp(0.1, 0.9);
        }
        read_ratios(first, &subs.objectAtIndex(0));
        read_ratios(second, &subs.objectAtIndex(1));
    }
}

/// The pane's foreground program if it isn't a shell.
fn program_of(term: &TermView) -> Option<String> {
    let name = term.foreground_name()?;
    let base = name.trim_start_matches('-');
    (!SHELLS.contains(&base)).then(|| base.to_string())
}

fn list_running(names: &[String]) -> String {
    match names {
        [one] => format!("{one} is"),
        _ => format!("{} are", names.join(", ")),
    }
}

pub fn tilde(dir: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    }
}
