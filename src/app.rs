//! The code viewer: file tree on the left, tab strip + editor on the right,
//! and a ⌘P quick-open panel. One object serves every AppKit callback.
//!
//! It runs standalone (`odek <folder>`, its own window and menus, as the app
//! delegate) or embedded as a pane in the terminal's workspace window. It is
//! an NSViewController so that, embedded, AppKit puts it in the responder
//! chain after its view and menu commands reach it while it has focus.

use std::cell::{Cell, OnceCell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class,
    msg_send, sel,
};
use objc2_app_kit::*;
use objc2_foundation::*;

#[path = "gitbar.rs"]
mod gitbar;
#[cfg(feature = "selftest")]
#[path = "selftest.rs"]
mod selftest;
#[path = "vimglue.rs"]
mod vimglue;

use crate::edit;
use crate::fuzzy;
use crate::highlight::{self, Lang, Span};
use crate::ruler::LineRuler;
use crate::tree::{ROOT, Tree};

pub const APP_NAME: &str = "Odek";

/// Keep at most this many files in memory; the least recently used clean
/// tab is closed when a new one opens.
const MAX_TABS: usize = 8;
/// Above this, skip syntax highlighting.
const HIGHLIGHT_LIMIT: u64 = 2 * 1024 * 1024;
/// Above this, show only the first `TRUNCATE_TO` bytes, read-only.
const HUGE_FILE: u64 = 20 * 1024 * 1024;
const TRUNCATE_TO: usize = 5 * 1024 * 1024;
const TAB_BAR_HEIGHT: f64 = 30.0;
const DEFAULT_FONT_SIZE: f64 = 12.5;
const STATUS_BAR_HEIGHT: f64 = 22.0;

define_class!(
    /// Root of an embedded viewer: paints the window background, which a
    /// window would otherwise provide behind the tab strip and status bar.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    struct Backdrop;

    impl Backdrop {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, rect: NSRect) {
            NSColor::windowBackgroundColor().setFill();
            NSRectFill(rect);
        }
    }
);

/// What an embedded viewer tells its owner.
pub enum ViewerEvent {
    /// The title changed; read it with `display_title`.
    Title,
    /// The editor or file tree took keyboard focus.
    Focused,
    /// ⌘W with no file open: the owner should close the viewer pane.
    Close,
}

type ViewerHandler = Box<dyn Fn(ViewerEvent)>;

thread_local! {
    /// The viewer, for work hopping back from background threads.
    static INSTANCE: OnceCell<Retained<App>> = const { OnceCell::new() };
}

/// Called by the editor view for every key; true if Vim mode consumed it.
pub fn vim_key_down(event: &NSEvent) -> bool {
    instance().is_some_and(|app| app.vim_key(event))
}

/// Called by the editor view when it becomes first responder.
pub fn editor_focused() {
    if let Some(app) = instance() {
        app.emit(ViewerEvent::Focused);
    }
}

fn instance() -> Option<Retained<App>> {
    INSTANCE.with(|i| i.get().cloned())
}

struct Tab {
    path: PathBuf,
    storage: Retained<NSTextStorage>,
    undo: Retained<NSUndoManager>,
    lang: Option<Lang>,
    spans: Vec<Span>,
    dirty: bool,
    preview: bool,
    read_only: bool,
    wrap: bool,
    mtime: Option<SystemTime>,
    selection: NSRange,
    /// Clip-view origin to restore; `None` = top-left of a fresh tab.
    scroll: Option<NSPoint>,
    last_used: u64,
}

#[derive(Default)]
struct Tabs {
    list: Vec<Tab>,
    current: Option<usize>,
    clock: u64,
}

#[derive(Default)]
struct Quick {
    files: Vec<String>,
    hits: Vec<usize>,
    built: Option<Instant>,
    /// Recently opened files (relative), newest first, shown for an empty query.
    recent: Vec<String>,
}

struct Ui {
    /// Its own window when standalone; None when embedded in a pane.
    window: Option<Retained<NSWindow>>,
    /// Everything below lives in this view: the window's content view, or
    /// the pane container.
    root: Retained<NSView>,
    split: Retained<NSSplitView>,
    outline: Retained<NSOutlineView>,
    tab_bar: Retained<NSStackView>,
    scroll: Retained<NSScrollView>,
    text: Retained<NSTextView>,
    code: Retained<crate::codeview::CodeView>,
    layout: Retained<NSLayoutManager>,
    container: Retained<NSTextContainer>,
    ruler: Retained<LineRuler>,
    placeholder: Retained<NSTextStorage>,
    placeholder_undo: Retained<NSUndoManager>,
    panel: Retained<NSPanel>,
    field: Retained<NSTextField>,
    results: Retained<NSTableView>,
    colors: Vec<Retained<NSColor>>,
    git_button: Retained<NSButton>,
    vim_label: Retained<NSTextField>,
}

pub struct Ivars {
    ui: OnceCell<Ui>,
    tree: RefCell<Option<Tree>>,
    tabs: RefCell<Tabs>,
    quick: RefCell<Quick>,
    font_size: Cell<f64>,
    #[cfg(feature = "selftest")]
    selftest: RefCell<Option<selftest::SelfTest>>,
    git: RefCell<gitbar::GitState>,
    vim: RefCell<crate::vim::Vim>,
    vim_on: Cell<bool>,
    on_event: RefCell<Option<ViewerHandler>>,
    /// File name or project folder, without the unsaved dot.
    title: RefCell<String>,
}

define_class!(
    #[unsafe(super(NSViewController, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct App;

    unsafe impl NSObjectProtocol for App {}
    unsafe impl NSApplicationDelegate for App {}
    unsafe impl NSWindowDelegate for App {}
    unsafe impl NSOutlineViewDataSource for App {}
    unsafe impl NSOutlineViewDelegate for App {}
    unsafe impl NSTableViewDataSource for App {}
    unsafe impl NSTableViewDelegate for App {}
    unsafe impl NSTextDelegate for App {}
    unsafe impl NSTextViewDelegate for App {}
    unsafe impl NSControlTextEditingDelegate for App {}
    unsafe impl NSTextFieldDelegate for App {}

    impl App {
        // ---- application lifecycle ----

        #[unsafe(method(applicationWillFinishLaunching:))]
        fn will_finish_launching(&self, _n: &NSNotification) {
            self.build_ui();
        }

        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            if self.ivars().tree.borrow().is_none() {
                let arg = std::env::args().skip(1).find(|a| !a.starts_with('-'));
                let last = NSUserDefaults::standardUserDefaults()
                    .stringForKey(ns_string!("lastFolder"))
                    .map(|s| s.to_string());
                match arg.map(PathBuf::from).or(last.map(PathBuf::from)) {
                    Some(p) if p.exists() => self.open_path(&p),
                    _ => self.choose_folder(),
                }
            }
            if let Some(w) = &self.ui().window {
                w.makeKeyAndOrderFront(None);
            }
            NSApplication::sharedApplication(self.mtm()).activate();
            #[cfg(feature = "selftest")]
            self.start_self_test();
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            if let Some(path) = urls.firstObject().and_then(|u| u.to_file_path()) {
                self.open_path(&path);
            }
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_last_window(&self, _app: &NSApplication) -> bool {
            true
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            if self.confirm_discard_all() {
                NSApplicationTerminateReply::TerminateNow
            } else {
                NSApplicationTerminateReply::TerminateCancel
            }
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _n: &NSNotification) {
            self.refresh_from_disk();
            self.git_on_activate();
        }

        // ---- windows ----

        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, sender: &NSWindow) -> bool {
            ptr_eq(sender, &*self.ui().panel) || self.confirm_discard_all()
        }

        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, n: &NSNotification) {
            let ui = self.ui();
            if n.object().is_some_and(|o| ptr_eq(&*o, &*ui.panel)) {
                ui.panel.orderOut(None);
            }
        }

        // ---- file tree (cell-based NSOutlineView; items are NSNumber node ids) ----

        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn outline_count(&self, _ov: &NSOutlineView, item: Option<&AnyObject>) -> NSInteger {
            let mut tree = self.ivars().tree.borrow_mut();
            match tree.as_mut() {
                Some(t) => t.children(node_of(item)).len() as NSInteger,
                None => 0,
            }
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn outline_child(&self, _ov: &NSOutlineView, index: NSInteger, item: Option<&AnyObject>) -> Retained<NSNumber> {
            let mut tree = self.ivars().tree.borrow_mut();
            let idx = tree
                .as_mut()
                .and_then(|t| t.children(node_of(item)).get(index as usize).copied())
                .unwrap_or(ROOT);
            NSNumber::new_usize(idx)
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn outline_expandable(&self, _ov: &NSOutlineView, item: &AnyObject) -> bool {
            let tree = self.ivars().tree.borrow();
            tree.as_ref().is_some_and(|t| t.node(node_of(Some(item))).is_dir)
        }

        #[unsafe(method_id(outlineView:objectValueForTableColumn:byItem:))]
        fn outline_value(&self, _ov: &NSOutlineView, _col: Option<&NSTableColumn>, item: Option<&AnyObject>) -> Option<Retained<NSAttributedString>> {
            let tree = self.ivars().tree.borrow();
            tree.as_ref().map(|t| tree_label(t.node(node_of(item))))
        }

        #[unsafe(method(outlineView:shouldEditTableColumn:item:))]
        fn outline_should_edit(&self, _ov: &NSOutlineView, _col: Option<&NSTableColumn>, _item: &AnyObject) -> bool {
            false
        }

        #[unsafe(method(treeClicked:))]
        fn tree_clicked(&self, _sender: Option<&AnyObject>) {
            self.emit(ViewerEvent::Focused);
            self.tree_activate(false);
        }

        #[unsafe(method(treeDoubleClicked:))]
        fn tree_double_clicked(&self, _sender: Option<&AnyObject>) {
            self.tree_activate(true);
        }

        // ---- quick-open results table ----

        #[unsafe(method(numberOfRowsInTableView:))]
        fn table_rows(&self, _tv: &NSTableView) -> NSInteger {
            self.ivars().quick.borrow().hits.len() as NSInteger
        }

        #[unsafe(method_id(tableView:objectValueForTableColumn:row:))]
        fn table_value(&self, _tv: &NSTableView, _col: Option<&NSTableColumn>, row: NSInteger) -> Option<Retained<NSAttributedString>> {
            let quick = self.ivars().quick.borrow();
            quick.hits.get(row as usize).and_then(|&i| quick.files.get(i)).map(|rel| result_label(rel))
        }

        #[unsafe(method(tableView:shouldEditTableColumn:row:))]
        fn table_should_edit(&self, _tv: &NSTableView, _col: Option<&NSTableColumn>, _row: NSInteger) -> bool {
            false
        }

        #[unsafe(method(quickPicked:))]
        fn quick_picked(&self, _sender: Option<&AnyObject>) {
            self.quick_open_selected();
        }

        // ---- quick-open search field ----

        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _n: &NSNotification) {
            self.quick_search();
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn control_command(&self, _control: &NSControl, _tv: &NSTextView, cmd: Sel) -> bool {
            self.quick_command(cmd)
        }

        // ---- editor ----

        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, _n: &NSNotification) {
            let ui = self.ui();
            ui.ruler.text_changed();
            let (relabel, len) = {
                let mut tabs = self.ivars().tabs.borrow_mut();
                let Some(i) = tabs.current else { return };
                let tab = &mut tabs.list[i];
                let relabel = !tab.dirty || tab.preview;
                tab.dirty = true;
                tab.preview = false;
                (relabel, tab.storage.length())
            };
            if relabel {
                self.rebuild_tab_bar();
            }
            // Re-highlight once typing pauses.
            let delay = if len > 300_000 { 0.4 } else { 0.15 };
            unsafe {
                let _: () = msg_send![NSObject::class(), cancelPreviousPerformRequestsWithTarget: self, selector: sel!(rehighlight:), object: None::<&AnyObject>];
                let _: () = msg_send![self, performSelector: sel!(rehighlight:), withObject: None::<&AnyObject>, afterDelay: delay];
            }
        }

        #[unsafe(method(rehighlight:))]
        fn rehighlight(&self, _arg: Option<&AnyObject>) {
            let current = self.ivars().tabs.borrow().current;
            if let Some(i) = current {
                self.highlight_tab(i);
            }
            self.apply_highlight();
        }

        #[unsafe(method_id(undoManagerForTextView:))]
        fn undo_manager_for(&self, _tv: &NSTextView) -> Option<Retained<NSUndoManager>> {
            let tabs = self.ivars().tabs.borrow();
            Some(match tabs.current {
                Some(i) => tabs.list[i].undo.clone(),
                None => self.ui().placeholder_undo.clone(),
            })
        }

        #[unsafe(method(textView:doCommandBySelector:))]
        fn text_command(&self, tv: &NSTextView, cmd: Sel) -> bool {
            self.editor_command(tv, cmd)
        }

        // ---- tab strip ----

        #[unsafe(method(tabClicked:))]
        fn tab_clicked(&self, sender: &NSButton) {
            self.activate_tab(sender.tag() as usize);
        }

        #[unsafe(method(tabCloseClicked:))]
        fn tab_close_clicked(&self, sender: &NSButton) {
            self.close_tab(sender.tag() as usize);
        }

        // ---- menu actions ----

        #[unsafe(method(appOpenFolder:))]
        fn menu_open_folder(&self, _sender: Option<&AnyObject>) {
            self.choose_folder();
        }

        #[unsafe(method(appQuickOpen:))]
        fn menu_quick_open(&self, _sender: Option<&AnyObject>) {
            self.show_quick_open();
        }

        #[unsafe(method(appSave:))]
        fn menu_save(&self, _sender: Option<&AnyObject>) {
            let current = self.ivars().tabs.borrow().current;
            if let Some(i) = current {
                self.save_tab(i);
            }
        }

        #[unsafe(method(appCloseTab:))]
        fn menu_close_tab(&self, _sender: Option<&AnyObject>) {
            self.close_current();
        }

        // ---- embedded: commands from the terminal's menus ----

        #[unsafe(method(loadView))]
        fn load_view(&self) {
            // Never load a nib; embedding sets the view explicitly.
            self.setView(&NSView::new(self.mtm()));
        }

        #[unsafe(method(termClosePane:))]
        fn term_close_pane(&self, _sender: Option<&AnyObject>) {
            self.close_current();
        }

        #[unsafe(method(termFind:))]
        fn term_find(&self, _sender: Option<&AnyObject>) {
            self.finder(NSTextFinderAction::ShowFindInterface);
        }

        #[unsafe(method(termFindNext:))]
        fn term_find_next(&self, _sender: Option<&AnyObject>) {
            self.finder(NSTextFinderAction::NextMatch);
        }

        #[unsafe(method(termFindPrevious:))]
        fn term_find_previous(&self, _sender: Option<&AnyObject>) {
            self.finder(NSTextFinderAction::PreviousMatch);
        }

        #[unsafe(method(termZoomIn:))]
        fn term_zoom_in(&self, _sender: Option<&AnyObject>) {
            self.set_font_size(self.ivars().font_size.get() + 1.0);
        }

        #[unsafe(method(termZoomOut:))]
        fn term_zoom_out(&self, _sender: Option<&AnyObject>) {
            self.set_font_size(self.ivars().font_size.get() - 1.0);
        }

        #[unsafe(method(termZoomReset:))]
        fn term_zoom_reset(&self, _sender: Option<&AnyObject>) {
            self.set_font_size(DEFAULT_FONT_SIZE);
        }

        #[unsafe(method(termShowFiles:))]
        fn term_show_files(&self, _sender: Option<&AnyObject>) {
            self.toggle_tree();
        }

        #[unsafe(method(appToggleSidebar:))]
        fn menu_toggle_sidebar(&self, _sender: Option<&AnyObject>) {
            self.toggle_tree();
        }

        #[unsafe(method(appToggleWrap:))]
        fn menu_toggle_wrap(&self, _sender: Option<&AnyObject>) {
            let wrap = {
                let mut tabs = self.ivars().tabs.borrow_mut();
                let Some(i) = tabs.current else { return };
                tabs.list[i].wrap = !tabs.list[i].wrap;
                tabs.list[i].wrap
            };
            self.set_wrap(wrap);
        }

        #[unsafe(method(appZoomIn:))]
        fn menu_zoom_in(&self, _sender: Option<&AnyObject>) {
            self.set_font_size(self.ivars().font_size.get() + 1.0);
        }

        #[unsafe(method(appZoomOut:))]
        fn menu_zoom_out(&self, _sender: Option<&AnyObject>) {
            self.set_font_size(self.ivars().font_size.get() - 1.0);
        }

        #[unsafe(method(appZoomReset:))]
        fn menu_zoom_reset(&self, _sender: Option<&AnyObject>) {
            self.set_font_size(DEFAULT_FONT_SIZE);
        }

        #[unsafe(method(appNextTab:))]
        fn menu_next_tab(&self, _sender: Option<&AnyObject>) {
            self.cycle_tab(1);
        }

        #[unsafe(method(appPrevTab:))]
        fn menu_prev_tab(&self, _sender: Option<&AnyObject>) {
            self.cycle_tab(-1);
        }

        #[unsafe(method(appGoToLine:))]
        fn menu_go_to_line(&self, _sender: Option<&AnyObject>) {
            self.go_to_line();
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            // A terminal renames ⌘/ to Keyboard Shortcuts; here it comments.
            if item.action() == Some(sel!(appToggleComment:)) {
                item.setTitle(ns_string!("Toggle Line Comment"));
            }
            // A terminal calls ⌘O New Tab in Folder; here it opens a folder.
            if item.action() == Some(sel!(appOpenFolder:)) {
                item.setTitle(ns_string!("Open Folder…"));
            }
            true
        }

        #[unsafe(method(appToggleComment:))]
        fn menu_toggle_comment(&self, _sender: Option<&AnyObject>) {
            self.toggle_comment();
        }

        #[unsafe(method(appToggleVim:))]
        fn menu_toggle_vim(&self, sender: Option<&NSMenuItem>) {
            self.vim_toggle(sender);
        }

        #[unsafe(method(gitMenu:))]
        fn git_menu(&self, _sender: Option<&AnyObject>) {
            self.git_show_menu();
        }

        #[unsafe(method(gitFetch:))]
        fn git_fetch(&self, _sender: Option<&AnyObject>) {
            self.git_fetch_now();
        }

        #[unsafe(method(gitPull:))]
        fn git_pull(&self, _sender: Option<&AnyObject>) {
            self.git_pull_now();
        }

        #[cfg(feature = "selftest")]
        #[unsafe(method(selfTestStep:))]
        fn self_test_step(&self, _arg: Option<&AnyObject>) {
            self.run_self_test_step();
        }

        #[unsafe(method(appRevealInFinder:))]
        fn menu_reveal(&self, _sender: Option<&AnyObject>) {
            let tabs = self.ivars().tabs.borrow();
            if let Some(i) = tabs.current {
                let url = NSURL::from_file_path(&tabs.list[i].path);
                if let Some(url) = url {
                    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
                }
            }
        }
    }
);

fn selected_range(tv: &NSTextView) -> NSRange {
    unsafe { msg_send![tv, selectedRange] }
}

fn ptr_eq<A: ?Sized, B: ?Sized>(a: &A, b: &B) -> bool {
    std::ptr::addr_eq(a as *const A, b as *const B)
}

fn node_of(item: Option<&AnyObject>) -> usize {
    item.and_then(|o| o.downcast_ref::<NSNumber>())
        .map(|n| n.as_usize())
        .unwrap_or(ROOT)
}

fn attrs(
    pairs: &[(&NSAttributedStringKey, &AnyObject)],
) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    let keys: Vec<&NSAttributedStringKey> = pairs.iter().map(|p| p.0).collect();
    let vals: Vec<&AnyObject> = pairs.iter().map(|p| p.1).collect();
    NSDictionary::from_slices(&keys, &vals)
}

/// Sidebar row text. The colour is baked in: the source-list style ignores
/// a cell text colour set in willDisplayCell for some rows.
fn tree_label(node: &crate::tree::Node) -> Retained<NSAttributedString> {
    let color = if node.ignored {
        NSColor::tertiaryLabelColor()
    } else {
        NSColor::labelColor()
    };
    let a = attrs(&[
        (unsafe { NSFontAttributeName }, &*NSFont::systemFontOfSize(12.5)),
        (unsafe { NSForegroundColorAttributeName }, &*color),
    ]);
    unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(&node.name),
            Some(&a),
        )
    }
}

/// "Menu.tsx   web/src/pages/admin" — name first, folder dimmed.
fn result_label(rel: &str) -> Retained<NSAttributedString> {
    let (dir, name) = match rel.rfind('/') {
        Some(i) => (&rel[..i], &rel[i + 1..]),
        None => ("", rel),
    };
    let out = NSMutableAttributedString::new();
    let name_attrs = attrs(&[
        (unsafe { NSFontAttributeName }, &*NSFont::systemFontOfSize(13.0)),
        (unsafe { NSForegroundColorAttributeName }, &*NSColor::labelColor()),
    ]);
    let dir_attrs = attrs(&[
        (unsafe { NSFontAttributeName }, &*NSFont::systemFontOfSize(11.5)),
        (
            unsafe { NSForegroundColorAttributeName },
            &*NSColor::secondaryLabelColor(),
        ),
    ]);
    unsafe {
        out.appendAttributedString(&NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(name),
            Some(&name_attrs),
        ));
        out.appendAttributedString(&NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(&format!("   {dir}")),
            Some(&dir_attrs),
        ));
    }
    Retained::into_super(out)
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

/// File contents ready for the editor.
struct Loaded {
    text: String,
    read_only: bool,
    highlight: bool,
}

fn load_file(path: &Path) -> std::io::Result<Loaded> {
    let size = fs::metadata(path)?.len();
    let mut bytes = fs::read(path)?;
    let mut read_only = false;
    if size > HUGE_FILE {
        bytes.truncate(TRUNCATE_TO);
        read_only = true;
    }
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Ok(Loaded {
            text: format!("Binary file ({} KB), not shown.", size / 1024),
            read_only: true,
            highlight: false,
        });
    }
    let mut text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => {
            read_only = true;
            String::from_utf8_lossy(e.as_bytes()).into_owned()
        }
    };
    if size > HUGE_FILE {
        text.push_str(&format!(
            "\n\n── Truncated: showing the first {} MB of {} MB (read-only). ──\n",
            TRUNCATE_TO / 1024 / 1024,
            size / 1024 / 1024
        ));
    }
    Ok(Loaded {
        text,
        read_only,
        highlight: size <= HIGHLIGHT_LIMIT,
    })
}

/// Nearest folder at or above `dir` holding a git repository, skipping the
/// home folder (a dotfiles repo there would make everything one project).
fn project_root(dir: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    dir.ancestors()
        .find(|d| Some(*d) != home.as_deref() && d.join(".git").exists())
        .map(Path::to_path_buf)
}

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn comment_prefix(lang: Option<Lang>) -> Option<&'static str> {
    Some(match lang? {
        Lang::Go | Lang::TypeScript | Lang::Tsx | Lang::JavaScript | Lang::Rust | Lang::Swift => "//",
        Lang::Python | Lang::Bash | Lang::Yaml => "#",
        Lang::Sql => "--",
        Lang::Latex => "%",
        Lang::Json | Lang::Markdown | Lang::Css | Lang::Html => return None,
    })
}

impl App {
    // ------------------------------------------------------------ embedding

    /// A viewer to embed in a pane; its view is `root_view()`.
    pub fn new_embedded(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::new(mtm);
        let root: Retained<Backdrop> = unsafe { msg_send![Backdrop::alloc(mtm), initWithFrame: frame] };
        // Without this (macOS 14+) the fill spills over the pane header.
        if root.respondsToSelector(objc2::sel!(setClipsToBounds:)) {
            root.setClipsToBounds(true);
        }
        let root = Retained::into_super(root);
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        this.build_into(&root, None);
        this.setView(&root);
        // In a pane the file tree starts hidden; ⇧⌘E shows it.
        let ui = this.ui();
        ui.split.subviews().objectAtIndex(0).setHidden(true);
        ui.split.adjustSubviews();
        this
    }

    pub fn root_view(&self) -> Retained<NSView> {
        self.ui().root.clone()
    }

    pub fn set_on_event(&self, f: impl Fn(ViewerEvent) + 'static) {
        *self.ivars().on_event.borrow_mut() = Some(Box::new(f));
    }

    fn emit(&self, e: ViewerEvent) {
        if let Some(f) = self.ivars().on_event.borrow().as_ref() {
            f(e);
        }
    }

    /// The window the viewer is in: its own, or the one hosting its pane.
    fn host_window(&self) -> Option<Retained<NSWindow>> {
        let ui = self.ui();
        ui.window.clone().or_else(|| ui.root.window())
    }

    fn set_title(&self, title: &str, file: Option<&Path>) {
        *self.ivars().title.borrow_mut() = title.to_string();
        match &self.ui().window {
            Some(w) => {
                w.setTitle(&NSString::from_str(title));
                w.setRepresentedURL(file.and_then(NSURL::from_file_path).as_deref());
            }
            None => self.emit(ViewerEvent::Title),
        }
    }

    /// Title with ● when a file has unsaved changes.
    pub fn display_title(&self) -> String {
        let dirty = self.ivars().tabs.borrow().list.iter().any(|t| t.dirty);
        let title = self.ivars().title.borrow();
        if dirty {
            format!("● {title}")
        } else {
            title.clone()
        }
    }

    pub fn root_dir(&self) -> Option<PathBuf> {
        self.ivars()
            .tree
            .borrow()
            .as_ref()
            .map(|t| t.root().to_path_buf())
    }

    /// Switch the project to `root` unless it is already open. False if the
    /// user cancelled (unsaved changes).
    fn ensure_root(&self, root: &Path) -> bool {
        if self.root_dir().as_deref() == Some(root) {
            return true;
        }
        self.set_root(root.to_path_buf())
    }

    /// Open `path` (a file, or a folder for the tree), at a 1-based line and
    /// column. Its project is the nearest git repository, else `near` (the
    /// terminal's folder) when it contains the file, else the file's folder.
    pub fn open_location(&self, path: &Path, line: Option<u32>, col: Option<u32>, near: Option<&Path>) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if path.is_dir() {
            return self.show_tree_in(&path);
        }
        let inside = self.root_dir().is_some_and(|r| path.starts_with(r));
        if !inside {
            let parent = path.parent().unwrap_or(&path);
            let root = project_root(parent).unwrap_or_else(|| match near {
                Some(n) if path.starts_with(n) => n.to_path_buf(),
                _ => parent.to_path_buf(),
            });
            if !self.set_root(root) {
                return;
            }
        }
        self.open_file(&path, false);
        self.reveal(&path);
        if let Some(line) = line {
            self.go_to(line as usize, col.unwrap_or(1) as usize);
        }
    }

    /// ⌘P from a terminal: quick open in that folder's project.
    pub fn quick_open_in(&self, dir: &Path) {
        let root = project_root(dir).unwrap_or_else(|| dir.to_path_buf());
        if self.ensure_root(&root) {
            self.show_quick_open();
        }
    }

    /// Show the file tree for a folder's project, with the folder revealed.
    pub fn show_tree_in(&self, dir: &Path) {
        let root = project_root(dir).unwrap_or_else(|| dir.to_path_buf());
        if !self.ensure_root(&root) {
            return;
        }
        let ui = self.ui();
        let side = ui.split.subviews().objectAtIndex(0);
        if side.isHidden() {
            side.setHidden(false);
            ui.split.adjustSubviews();
        }
        if dir != root {
            self.reveal(dir);
        }
        if let Some(w) = self.host_window() {
            w.makeFirstResponder(Some(&ui.outline));
        }
    }

    /// Selftest: where the editor is scrolled, for checking jumps.
    #[cfg(feature = "selftest")]
    pub fn debug_scroll(&self) -> String {
        let ui = self.ui();
        let clip = ui.scroll.contentView();
        let b = clip.bounds();
        let i = clip.contentInsets();
        format!(
            "clip origin ({:.1},{:.1}) size ({:.1}x{:.1}) insets left {:.1} top {:.1} ruler {:.1} visible {}",
            b.origin.x,
            b.origin.y,
            b.size.width,
            b.size.height,
            i.left,
            i.top,
            ui.ruler.ruleThickness(),
            ui.scroll.rulersVisible()
        )
    }

    /// Keyboard focus to the editor, or the tree when no file is open.
    pub fn focus(&self) {
        let ui = self.ui();
        let Some(w) = self.host_window() else { return };
        if self.ivars().tabs.borrow().current.is_some() {
            w.makeFirstResponder(Some(&ui.text));
        } else {
            w.makeFirstResponder(Some(&ui.outline));
        }
    }

    /// Close every file (after `confirm_discard_all`) to free their memory.
    pub fn release_files(&self) {
        {
            let mut tabs = self.ivars().tabs.borrow_mut();
            tabs.list = Vec::new();
            tabs.current = None;
        }
        *self.ivars().quick.borrow_mut() = Quick::default();
        self.ui().panel.orderOut(None);
        self.show_placeholder();
        self.rebuild_tab_bar();
    }

    /// The app became active again: pick up changes on disk, fetch git.
    pub fn app_activated(&self) {
        if self.ivars().ui.get().is_some() {
            self.refresh_from_disk();
            self.git_on_activate();
        }
    }

    fn toggle_tree(&self) {
        let ui = self.ui();
        let side = &ui.split.subviews().objectAtIndex(0);
        side.setHidden(!side.isHidden());
        ui.split.adjustSubviews();
    }

    /// ⌘W: hide quick open, else close the current file, else the window/pane.
    fn close_current(&self) {
        let ui = self.ui();
        if NSApplication::sharedApplication(self.mtm())
            .keyWindow()
            .is_some_and(|w| ptr_eq(&*w, &*ui.panel))
        {
            ui.panel.orderOut(None);
            return;
        }
        let current = self.ivars().tabs.borrow().current;
        match (current, &ui.window) {
            (Some(i), _) => self.close_tab(i),
            (None, Some(w)) => w.performClose(None),
            (None, None) => self.emit(ViewerEvent::Close),
        }
    }

    fn finder(&self, action: NSTextFinderAction) {
        let item = NSMenuItem::new(self.mtm());
        item.setTag(action.0);
        let _: () = unsafe { msg_send![&*self.ui().text, performTextFinderAction: &*item] };
    }

    fn quick_command(&self, cmd: Sel) -> bool {
        let ui = self.ui();
        let rows = self.ivars().quick.borrow().hits.len() as NSInteger;
        let row = ui.results.selectedRow();
        let go = |r: NSInteger| {
            if rows > 0 {
                let r = r.clamp(0, rows - 1);
                ui.results
                    .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(r as usize), false);
                ui.results.scrollRowToVisible(r);
            }
        };
        if cmd == sel!(moveDown:) {
            go(row + 1);
        } else if cmd == sel!(moveUp:) {
            go(row - 1);
        } else if cmd == sel!(insertNewline:) {
            self.quick_open_selected();
        } else if cmd == sel!(cancelOperation:) {
            ui.panel.orderOut(None);
        } else {
            return false;
        }
        true
    }

    fn editor_command(&self, tv: &NSTextView, cmd: Sel) -> bool {
        if cmd != sel!(insertNewline:) || !tv.isEditable() {
            return false;
        }
        // Keep the current line's indentation, like every code editor.
        let text = tv.string();
        let sel_range = selected_range(tv);
        let line = text.lineRangeForRange(NSRange::new(sel_range.location, 0));
        let upto = NSRange::new(line.location, sel_range.location - line.location);
        let prefix = text.substringWithRange(upto).to_string();
        let indent = edit::indent_of(&prefix);
        unsafe { tv.insertText_replacementRange(&NSString::from_str(&format!("\n{indent}")), sel_range) };
        true
    }

    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let size = NSUserDefaults::standardUserDefaults().doubleForKey(ns_string!("fontSize"));
        let this = Self::alloc(mtm).set_ivars(Ivars {
            ui: OnceCell::new(),
            tree: RefCell::new(None),
            tabs: RefCell::new(Tabs::default()),
            quick: RefCell::new(Quick::default()),
            font_size: Cell::new(if size >= 8.0 { size } else { DEFAULT_FONT_SIZE }),
            #[cfg(feature = "selftest")]
            selftest: RefCell::new(selftest::SelfTest::from_env()),
            git: RefCell::new(gitbar::GitState::default()),
            vim: RefCell::new(crate::vim::Vim::new()),
            vim_on: Cell::new(NSUserDefaults::standardUserDefaults().boolForKey(ns_string!("vimMode"))),
            on_event: RefCell::new(None),
            title: RefCell::new(String::new()),
        });
        let this: Retained<Self> =
            unsafe { msg_send![super(this), initWithNibName: None::<&NSString>, bundle: None::<&NSBundle>] };
        INSTANCE.with(|i| {
            let _ = i.set(this.clone());
        });
        this
    }

    fn ui(&self) -> &Ui {
        self.ivars().ui.get().expect("UI is built at launch")
    }

    fn font(&self) -> Retained<NSFont> {
        NSFont::monospacedSystemFontOfSize_weight(self.ivars().font_size.get(), unsafe {
            NSFontWeightRegular
        })
    }

    fn line_spacing(&self) -> f64 {
        (self.ivars().font_size.get() * 0.4).round()
    }

    /// Font, tab width (4 columns) and line spacing for editor text.
    fn base_attrs(&self) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
        let font = self.font();
        let width = unsafe {
            NSString::from_str("    ").sizeWithAttributes(Some(&attrs(&[(NSFontAttributeName, &*font)])))
        }
        .width;
        let para = NSMutableParagraphStyle::new();
        para.setDefaultTabInterval(width);
        para.setTabStops(Some(&NSArray::new()));
        para.setLineSpacing(self.line_spacing());
        unsafe {
            attrs(&[
                (NSFontAttributeName, &*font),
                (NSForegroundColorAttributeName, &*NSColor::textColor()),
                (NSParagraphStyleAttributeName, &*para),
            ])
        }
    }

    fn make_storage(&self, text: &str) -> Retained<NSTextStorage> {
        let attrs = self.base_attrs();
        unsafe {
            msg_send![NSTextStorage::alloc(), initWithString: &*NSString::from_str(text), attributes: &*attrs]
        }
    }

    // ---------------------------------------------------------------- UI build

    /// Standalone: the main menu and a window holding the viewer.
    fn build_ui(&self) {
        let mtm = self.mtm();
        let app = NSApplication::sharedApplication(mtm);
        app.setMainMenu(Some(&self.build_menu()));

        // Window
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, 1100.0, 720.0),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(APP_NAME));
        window.setMinSize(NSSize::new(480.0, 300.0));
        // sRGB: 8-bit window buffers instead of half-float (half the memory).
        window.setColorSpace(Some(&objc2_app_kit::NSColorSpace::sRGBColorSpace()));
        window.center();
        window.setFrameAutosaveName(ns_string!("main"));
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.setTabbingMode(NSWindowTabbingMode::Disallowed);

        let content = window.contentView().expect("window has a content view");
        self.build_into(&content, Some(window));
    }

    /// Build the viewer's views inside `content`.
    fn build_into(&self, content: &NSView, window: Option<Retained<NSWindow>>) {
        let mtm = self.mtm();
        let bounds = content.bounds();

        // Split: sidebar | editor
        let split = NSSplitView::initWithFrame(
            NSSplitView::alloc(mtm),
            rect(
                0.0,
                STATUS_BAR_HEIGHT,
                bounds.size.width,
                bounds.size.height - STATUS_BAR_HEIGHT,
            ),
        );
        split.setVertical(true);
        split.setDividerStyle(NSSplitViewDividerStyle::Thin);
        split.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        // Sidebar: outline view in a scroll view
        let side_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(0.0, 0.0, 240.0, bounds.size.height),
        );
        side_scroll.setHasVerticalScroller(true);
        side_scroll.setAutohidesScrollers(true);
        side_scroll.setDrawsBackground(false);
        let outline = NSOutlineView::initWithFrame(NSOutlineView::alloc(mtm), side_scroll.bounds());
        let col = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("name"));
        col.setEditable(false);
        col.setResizingMask(NSTableColumnResizingOptions::AutoresizingMask);
        outline.addTableColumn(&col);
        unsafe { outline.setOutlineTableColumn(Some(&col)) };
        outline.setHeaderView(None);
        outline.setStyle(NSTableViewStyle::SourceList);
        outline
            .setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle);
        outline.setIndentationPerLevel(12.0);
        outline.setAutoresizesOutlineColumn(false);
        unsafe {
            outline.setDataSource(Some(ProtocolObject::from_ref(self)));
            outline.setDelegate(Some(ProtocolObject::from_ref(self)));
            outline.setTarget(Some(self));
            outline.setAction(Some(sel!(treeClicked:)));
            outline.setDoubleAction(Some(sel!(treeDoubleClicked:)));
        }
        side_scroll.setDocumentView(Some(&outline));
        col.setWidth(side_scroll.contentSize().width);
        outline.sizeLastColumnToFit();
        if let Ok(cell) = col.dataCell().downcast::<NSTextFieldCell>() {
            cell.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
            cell.setFont(Some(&NSFont::systemFontOfSize(12.5)));
        }

        // Editor side: tab strip on top, text below
        let editor_side =
            NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 860.0, bounds.size.height));
        let h = bounds.size.height;
        let tab_bar = NSStackView::new(mtm);
        tab_bar.setFrame(rect(0.0, h - TAB_BAR_HEIGHT, 860.0, TAB_BAR_HEIGHT));
        tab_bar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        tab_bar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        tab_bar.setSpacing(0.0);
        tab_bar.setAlignment(NSLayoutAttribute::CenterY);
        tab_bar.setEdgeInsets(NSEdgeInsets {
            top: 0.0,
            left: 6.0,
            bottom: 0.0,
            right: 6.0,
        });
        tab_bar.setDistribution(NSStackViewDistribution::GravityAreas);
        editor_side.addSubview(&tab_bar);

        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(0.0, 0.0, 860.0, h - TAB_BAR_HEIGHT),
        );
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);

        // TextKit 1 stack, built by hand so tabs can swap NSTextStorage.
        let placeholder = self.make_storage(
            "\n    ⌘P   Quick open a file\n    ⌘O   Open folder\n    ⌘B   Toggle sidebar\n    ⌘F   Find in file\n",
        );
        let layout = NSLayoutManager::new();
        layout.setAllowsNonContiguousLayout(true);
        placeholder.addLayoutManager(&layout);
        let container =
            NSTextContainer::initWithContainerSize(NSTextContainer::alloc(), NSSize::new(f64::MAX, f64::MAX));
        container.setWidthTracksTextView(false);
        layout.addTextContainer(&container);
        let content_size = scroll.contentSize();
        let code = crate::codeview::CodeView::new(
            rect(0.0, 0.0, content_size.width, content_size.height),
            &container,
            mtm,
        );
        let text = crate::codeview::as_text_view(&code);
        text.setMinSize(NSSize::new(0.0, content_size.height));
        text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(true);
        text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        text.setRichText(false);
        text.setImportsGraphics(false);
        text.setAllowsUndo(true);
        text.setUsesFindBar(true);
        text.setIncrementalSearchingEnabled(true);
        text.setAutomaticQuoteSubstitutionEnabled(false);
        text.setAutomaticDashSubstitutionEnabled(false);
        text.setAutomaticTextReplacementEnabled(false);
        text.setAutomaticSpellingCorrectionEnabled(false);
        text.setContinuousSpellCheckingEnabled(false);
        text.setGrammarCheckingEnabled(false);
        text.setAutomaticLinkDetectionEnabled(false);
        text.setSmartInsertDeleteEnabled(false);
        text.setTextContainerInset(NSSize::new(4.0, 6.0));
        // The cursor blue from the icon (also the Vim block cursor).
        text.setInsertionPointColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0x3B as f64 / 255.0,
            0x82 as f64 / 255.0,
            0xF6 as f64 / 255.0,
            1.0,
        )));
        text.setEditable(false);
        text.setDelegate(Some(ProtocolObject::from_ref(self)));
        scroll.setDocumentView(Some(&text));

        let ruler = LineRuler::new(&scroll, self.ivars().font_size.get(), mtm);
        ruler.setClientView(Some(&text));
        ruler.set_metrics(self.ivars().font_size.get(), self.line_spacing());
        scroll.setVerticalRulerView(Some(&ruler));
        scroll.setHasVerticalRuler(true);
        scroll.setRulersVisible(false);
        // Redraw numbers while scrolling (NSRulerView doesn't always on its own).
        let clip = scroll.contentView();
        clip.setPostsBoundsChangedNotifications(true);
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &ruler,
                sel!(setNeedsDisplay:),
                Some(NSViewBoundsDidChangeNotification),
                Some(&clip),
            );
        }
        editor_side.addSubview(&scroll);

        split.addSubview(&side_scroll);
        split.addSubview(&editor_side);
        split.setAutosaveName(Some(ns_string!("split")));
        content.addSubview(&split);

        // Status bar along the bottom: git branch on the left.
        let status_bar = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, bounds.size.width, STATUS_BAR_HEIGHT),
        );
        status_bar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        let line = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            rect(0.0, STATUS_BAR_HEIGHT - 1.0, bounds.size.width, 1.0),
        );
        line.setBoxType(NSBoxType::Separator);
        line.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        status_bar.addSubview(&line);
        let git_button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!(""), Some(self), Some(sel!(gitMenu:)), mtm)
        };
        git_button.setBordered(false);
        git_button.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                ns_string!("arrow.triangle.branch"),
                Some(ns_string!("Git branch")),
            )
            .as_deref(),
        );
        git_button.setImagePosition(NSCellImagePosition::ImageLeading);
        git_button.setFrameOrigin(NSPoint::new(8.0, 2.0));
        git_button.setRefusesFirstResponder(true);
        git_button.setHidden(true);
        status_bar.addSubview(&git_button);
        let vim_label = NSTextField::labelWithString(ns_string!(""), mtm);
        vim_label.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(11.0, unsafe {
            NSFontWeightMedium
        })));
        vim_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        vim_label.setAlignment(NSTextAlignment::Right);
        vim_label.setFrame(rect(bounds.size.width - 412.0, 3.0, 400.0, 16.0));
        vim_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        status_bar.addSubview(&vim_label);
        content.addSubview(&status_bar);
        split.setPosition_ofDividerAtIndex(240.0, 0);

        // Quick-open panel
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            rect(0.0, 0.0, 620.0, 380.0),
            NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView,
            NSBackingStoreType::Buffered,
            true,
        );
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        panel.setTitlebarAppearsTransparent(true);
        for b in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(button) = panel.standardWindowButton(b) {
                button.setHidden(true);
            }
        }
        panel.setMovableByWindowBackground(true);
        panel.setHidesOnDeactivate(true);
        panel.setDelegate(Some(ProtocolObject::from_ref(self)));
        let pc = panel.contentView().expect("panel has a content view");
        let field = NSTextField::new(mtm);
        field.setFrame(rect(14.0, 380.0 - 14.0 - 30.0, 592.0, 30.0));
        field.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        field.setFont(Some(&NSFont::systemFontOfSize(16.0)));
        field.setPlaceholderString(Some(ns_string!("Search files by name")));
        field.setBezelStyle(NSTextFieldBezelStyle::RoundedBezel);
        field.setFocusRingType(NSFocusRingType::None);
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(self))) };
        pc.addSubview(&field);
        let rscroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0.0, 6.0, 620.0, 380.0 - 58.0));
        rscroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        rscroll.setHasVerticalScroller(true);
        rscroll.setAutohidesScrollers(true);
        rscroll.setDrawsBackground(false);
        let results = NSTableView::initWithFrame(NSTableView::alloc(mtm), rscroll.bounds());
        let rcol = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("file"));
        rcol.setEditable(false);
        rcol.setResizingMask(NSTableColumnResizingOptions::AutoresizingMask);
        results.addTableColumn(&rcol);
        results.setHeaderView(None);
        results.setStyle(NSTableViewStyle::Inset);
        results.setRowHeight(24.0);
        results.setBackgroundColor(&NSColor::clearColor());
        results
            .setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle);
        if let Ok(cell) = rcol.dataCell().downcast::<NSTextFieldCell>() {
            cell.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        }
        unsafe {
            results.setDataSource(Some(ProtocolObject::from_ref(self)));
            results.setDelegate(Some(ProtocolObject::from_ref(self)));
            results.setTarget(Some(self));
            results.setAction(Some(sel!(quickPicked:)));
        }
        rscroll.setDocumentView(Some(&results));
        rcol.setWidth(rscroll.contentSize().width);
        results.sizeLastColumnToFit();
        pc.addSubview(&rscroll);

        let colors = crate::theme::palette();

        let ui = Ui {
            window,
            root: content.retain(),
            split,
            outline,
            tab_bar,
            scroll,
            text,
            layout,
            container,
            ruler,
            placeholder,
            placeholder_undo: NSUndoManager::new(self.mtm()),
            panel,
            field,
            results,
            colors,
            git_button,
            vim_label,
            code,
        };
        let _ = self.ivars().ui.set(ui);
        self.vim_update_ui();
    }

    fn build_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let cmd = NSEventModifierFlags::Command;
        let shift = NSEventModifierFlags::Shift;
        let opt = NSEventModifierFlags::Option;
        let ctrl = NSEventModifierFlags::Control;
        let item = |title: &str, action: Option<Sel>, key: &str, mods: NSEventModifierFlags, tag: isize| {
            let it = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    action,
                    &NSString::from_str(key),
                )
            };
            it.setKeyEquivalentModifierMask(mods);
            it.setTag(tag);
            it
        };
        let menu = |title: &str, items: Vec<Retained<NSMenuItem>>| {
            let m = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
            for it in items {
                m.addItem(&it);
            }
            let holder = item(title, None, "", cmd, 0);
            holder.setSubmenu(Some(&m));
            holder
        };
        let sep = || NSMenuItem::separatorItem(mtm);
        let finder = |title: &str, key: &str, mods, action: NSTextFinderAction| {
            item(title, Some(sel!(performTextFinderAction:)), key, mods, action.0)
        };

        let bar = NSMenu::new(mtm);
        let items = vec![
            menu(
                APP_NAME,
                vec![
                    item(
                        &format!("About {APP_NAME}"),
                        Some(sel!(orderFrontStandardAboutPanel:)),
                        "",
                        cmd,
                        0,
                    ),
                    sep(),
                    item(&format!("Hide {APP_NAME}"), Some(sel!(hide:)), "h", cmd, 0),
                    item(
                        "Hide Others",
                        Some(sel!(hideOtherApplications:)),
                        "h",
                        cmd | opt,
                        0,
                    ),
                    item("Show All", Some(sel!(unhideAllApplications:)), "", cmd, 0),
                    sep(),
                    item(&format!("Quit {APP_NAME}"), Some(sel!(terminate:)), "q", cmd, 0),
                ],
            ),
            menu(
                "File",
                vec![
                    item("Open Folder…", Some(sel!(appOpenFolder:)), "o", cmd, 0),
                    item("Quick Open…", Some(sel!(appQuickOpen:)), "p", cmd, 0),
                    sep(),
                    item("Save", Some(sel!(appSave:)), "s", cmd, 0),
                    item("Close Tab", Some(sel!(appCloseTab:)), "w", cmd, 0),
                    sep(),
                    item(
                        "Reveal in Finder",
                        Some(sel!(appRevealInFinder:)),
                        "r",
                        cmd | opt,
                        0,
                    ),
                ],
            ),
            menu(
                "Edit",
                vec![
                    item("Undo", Some(sel!(undo:)), "z", cmd, 0),
                    item("Redo", Some(sel!(redo:)), "z", cmd | shift, 0),
                    sep(),
                    item("Cut", Some(sel!(cut:)), "x", cmd, 0),
                    item("Copy", Some(sel!(copy:)), "c", cmd, 0),
                    item("Paste", Some(sel!(paste:)), "v", cmd, 0),
                    item("Select All", Some(sel!(selectAll:)), "a", cmd, 0),
                    sep(),
                    item("Toggle Line Comment", Some(sel!(appToggleComment:)), "/", cmd, 0),
                    sep(),
                    finder("Find…", "f", cmd, NSTextFinderAction::ShowFindInterface),
                    finder(
                        "Replace…",
                        "f",
                        cmd | opt,
                        NSTextFinderAction::ShowReplaceInterface,
                    ),
                    finder("Find Next", "g", cmd, NSTextFinderAction::NextMatch),
                    finder(
                        "Find Previous",
                        "g",
                        cmd | shift,
                        NSTextFinderAction::PreviousMatch,
                    ),
                    finder(
                        "Use Selection for Find",
                        "e",
                        cmd,
                        NSTextFinderAction::SetSearchString,
                    ),
                ],
            ),
            menu(
                "View",
                vec![
                    item("Toggle Sidebar", Some(sel!(appToggleSidebar:)), "b", cmd, 0),
                    item("Toggle Word Wrap", Some(sel!(appToggleWrap:)), "z", opt, 0),
                    item("Vim Mode", Some(sel!(appToggleVim:)), "v", cmd | opt, 0),
                    sep(),
                    item("Zoom In", Some(sel!(appZoomIn:)), "=", cmd, 0),
                    item("Zoom Out", Some(sel!(appZoomOut:)), "-", cmd, 0),
                    item("Reset Zoom", Some(sel!(appZoomReset:)), "0", cmd, 0),
                ],
            ),
            menu(
                "Go",
                vec![
                    item("Go to File…", Some(sel!(appQuickOpen:)), "", cmd, 0),
                    item("Go to Line…", Some(sel!(appGoToLine:)), "g", ctrl, 0),
                    sep(),
                    item("Next Tab", Some(sel!(appNextTab:)), "]", cmd | shift, 0),
                    item("Previous Tab", Some(sel!(appPrevTab:)), "[", cmd | shift, 0),
                    item("Next Tab", Some(sel!(appNextTab:)), "\t", ctrl, 0),
                    item("Previous Tab", Some(sel!(appPrevTab:)), "\t", ctrl | shift, 0),
                ],
            ),
            menu(
                "Window",
                vec![
                    item("Minimize", Some(sel!(performMiniaturize:)), "m", cmd, 0),
                    item("Zoom", Some(sel!(performZoom:)), "", cmd, 0),
                ],
            ),
        ];
        // The ⌃Tab duplicates are alternates: keep them working but hidden.
        if let Some(go) = items[4].submenu() {
            for i in [5, 6] {
                if let Some(it) = go.itemAtIndex(i) {
                    it.setHidden(true);
                    it.setAllowsKeyEquivalentWhenHidden(true);
                }
            }
        }
        if let Some(vim_item) = items[3]
            .submenu()
            .and_then(|m| m.itemWithTitle(ns_string!("Vim Mode")))
        {
            let on = self.ivars().vim_on.get();
            vim_item.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        for it in items {
            bar.addItem(&it);
        }
        bar
    }

    // ------------------------------------------------------------- folders

    /// Open a folder, or a file (its folder becomes the root).
    fn open_path(&self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if path.is_dir() {
            self.set_root(path);
        } else {
            let inside = self
                .ivars()
                .tree
                .borrow()
                .as_ref()
                .is_some_and(|t| path.starts_with(t.root()));
            if !inside {
                let Some(parent) = path.parent() else { return };
                if !self.set_root(parent.to_path_buf()) {
                    return;
                }
            }
            self.open_file(&path, false);
            self.reveal(&path);
        }
    }

    fn choose_folder(&self) {
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(true);
        panel.setAllowsMultipleSelection(false);
        panel.setPrompt(Some(ns_string!("Open")));
        if panel.runModal() == NSModalResponseOK
            && let Some(path) = panel.URLs().firstObject().and_then(|u| u.to_file_path())
        {
            self.open_path(&path);
            // In a pane the file tree starts hidden; show what was opened.
            if self.ui().window.is_none() {
                let ui = self.ui();
                let side = ui.split.subviews().objectAtIndex(0);
                side.setHidden(false);
                ui.split.adjustSubviews();
            }
        }
    }

    /// Replace the project root. Returns false if the user cancelled.
    fn set_root(&self, root: PathBuf) -> bool {
        if !self.confirm_discard_all() {
            return false;
        }
        let ui = self.ui();
        {
            let mut tabs = self.ivars().tabs.borrow_mut();
            tabs.list.clear();
            tabs.current = None;
        }
        self.show_placeholder();
        self.rebuild_tab_bar();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        unsafe {
            NSUserDefaults::standardUserDefaults().setObject_forKey(
                Some(&NSString::from_str(&root.to_string_lossy())),
                ns_string!("lastFolder"),
            )
        };
        if let Some(url) = NSURL::from_directory_path(&root) {
            NSDocumentController::sharedDocumentController(self.mtm()).noteNewRecentDocumentURL(&url);
        }
        *self.ivars().tree.borrow_mut() = Some(Tree::new(root));
        *self.ivars().quick.borrow_mut() = Quick::default();
        ui.outline.reloadData();
        self.set_title(&name, None);
        self.git_project_changed();
        true
    }

    fn tree_activate(&self, pin: bool) {
        let ui = self.ui();
        let row = ui.outline.clickedRow();
        if row < 0 {
            return;
        }
        let Some(item) = ui.outline.itemAtRow(row) else {
            return;
        };
        let idx = node_of(Some(&item));
        let (is_dir, path) = {
            let tree = self.ivars().tree.borrow();
            let Some(t) = tree.as_ref() else { return };
            (t.node(idx).is_dir, t.node(idx).path.clone())
        };
        if is_dir {
            if !pin {
                if unsafe { ui.outline.isItemExpanded(Some(&item)) } {
                    unsafe {
                        ui.outline.collapseItem(Some(&item));
                    }
                } else {
                    unsafe {
                        ui.outline.expandItem(Some(&item));
                    }
                }
            }
        } else {
            self.open_file(&path, !pin);
        }
    }

    /// Expand the sidebar down to `path` and select it.
    fn reveal(&self, path: &Path) {
        let chain = {
            let mut tree = self.ivars().tree.borrow_mut();
            match tree.as_mut().and_then(|t| t.path_to(path)) {
                Some(c) => c,
                None => return,
            }
        };
        let ui = self.ui();
        for &idx in &chain[..chain.len().saturating_sub(1)] {
            unsafe {
                ui.outline.expandItem(Some(&NSNumber::new_usize(idx)));
            }
        }
        if let Some(&last) = chain.last() {
            let row = unsafe { ui.outline.rowForItem(Some(&NSNumber::new_usize(last))) };
            if row >= 0 {
                ui.outline.selectRowIndexes_byExtendingSelection(
                    &NSIndexSet::indexSetWithIndex(row as usize),
                    false,
                );
                ui.outline.scrollRowToVisible(row);
            }
        }
    }

    /// Pick up files created/deleted/changed outside the app.
    fn refresh_from_disk(&self) {
        let ui = self.ui();
        let changed = match self.ivars().tree.borrow_mut().as_mut() {
            Some(t) => t.refresh(),
            None => return,
        };
        for idx in changed {
            if idx == ROOT {
                ui.outline.reloadData();
                break;
            }
            unsafe {
                ui.outline
                    .reloadItem_reloadChildren(Some(&NSNumber::new_usize(idx)), true);
            }
        }
        self.ivars().quick.borrow_mut().built = None;

        // Reload clean tabs whose file changed on disk.
        let stale: Vec<usize> = {
            let tabs = self.ivars().tabs.borrow();
            tabs.list
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.dirty && t.mtime.is_some() && mtime(&t.path) != t.mtime)
                .map(|(i, _)| i)
                .collect()
        };
        for i in stale {
            let path = self.ivars().tabs.borrow().list[i].path.clone();
            if let Ok(loaded) = load_file(&path) {
                let storage = {
                    let mut tabs = self.ivars().tabs.borrow_mut();
                    let tab = &mut tabs.list[i];
                    tab.mtime = mtime(&path);
                    tab.read_only = loaded.read_only;
                    tab.storage.clone()
                };
                let is_current = self.ivars().tabs.borrow().current == Some(i);
                let selection = selected_range(&ui.text);
                storage.beginEditing();
                storage.replaceCharactersInRange_withString(
                    NSRange::new(0, storage.length()),
                    &NSString::from_str(&loaded.text),
                );
                unsafe {
                    storage.setAttributes_range(Some(&self.base_attrs()), NSRange::new(0, storage.length()));
                }
                storage.endEditing();
                if loaded.highlight {
                    self.highlight_tab(i);
                }
                if is_current {
                    ui.ruler.text_changed();
                    let len = storage.length();
                    ui.text
                        .setSelectedRange(NSRange::new(selection.location.min(len), 0));
                    self.apply_highlight();
                }
            }
        }
    }

    // ---------------------------------------------------------------- tabs

    fn open_file(&self, path: &Path, preview: bool) {
        // Already open? Just switch (and pin if asked).
        let existing = self
            .ivars()
            .tabs
            .borrow()
            .list
            .iter()
            .position(|t| t.path == path);
        if let Some(i) = existing {
            if !preview {
                self.ivars().tabs.borrow_mut().list[i].preview = false;
            }
            self.activate_tab(i);
            return;
        }
        let loaded = match load_file(path) {
            Ok(l) => l,
            Err(e) => {
                self.alert(&format!("Can't open {}", path.display()), &e.to_string(), &["OK"]);
                return;
            }
        };
        let lang = Lang::detect(path);
        let wrap = matches!(lang, Some(Lang::Markdown | Lang::Latex))
            || path.extension().is_some_and(|e| e == "txt");
        let tab = Tab {
            path: path.to_path_buf(),
            storage: self.make_storage(&loaded.text),
            undo: NSUndoManager::new(self.mtm()),
            lang: if loaded.highlight { lang } else { None },
            spans: Vec::new(),
            dirty: false,
            preview,
            read_only: loaded.read_only,
            wrap,
            mtime: mtime(path),
            selection: NSRange::new(0, 0),
            scroll: None,
            last_used: 0,
        };
        drop(loaded);

        let index = {
            let mut tabs = self.ivars().tabs.borrow_mut();
            // A clean preview tab gets replaced instead of adding another tab.
            let reuse = tabs.list.iter().position(|t| t.preview && !t.dirty);
            match reuse {
                Some(i) => {
                    tabs.list[i] = tab;
                    if tabs.current == Some(i) {
                        tabs.current = None; // force a fresh activate
                    }
                    i
                }
                None => {
                    let at = tabs.current.map_or(tabs.list.len(), |c| c + 1);
                    tabs.list.insert(at, tab);
                    if let Some(c) = tabs.current
                        && c >= at
                    {
                        tabs.current = Some(c + 1);
                    }
                    at
                }
            }
        };
        self.highlight_tab(index);
        self.remember_recent(path);
        self.activate_tab(index);
        self.enforce_tab_limit();
    }

    /// Close least-recently-used clean tabs beyond MAX_TABS.
    fn enforce_tab_limit(&self) {
        loop {
            let victim = {
                let tabs = self.ivars().tabs.borrow();
                if tabs.list.len() <= MAX_TABS {
                    return;
                }
                tabs.list
                    .iter()
                    .enumerate()
                    .filter(|(i, t)| !t.dirty && Some(*i) != tabs.current)
                    .min_by_key(|(_, t)| t.last_used)
                    .map(|(i, _)| i)
            };
            match victim {
                Some(i) => self.remove_tab(i),
                None => return,
            }
        }
    }

    fn activate_tab(&self, i: usize) {
        let ui = self.ui();
        let (storage, read_only, wrap, selection, scroll, path) = {
            let mut tabs = self.ivars().tabs.borrow_mut();
            if i >= tabs.list.len() {
                return;
            }
            if tabs.current == Some(i) {
                drop(tabs);
                self.rebuild_tab_bar();
                return;
            }
            // Remember where we were in the tab we're leaving.
            if let Some(c) = tabs.current
                && let Some(old) = tabs.list.get_mut(c)
            {
                old.selection = selected_range(&ui.text);
                old.scroll = Some(ui.scroll.contentView().bounds().origin);
            }
            tabs.clock += 1;
            let clock = tabs.clock;
            tabs.current = Some(i);
            let t = &mut tabs.list[i];
            t.last_used = clock;
            (
                t.storage.clone(),
                t.read_only,
                t.wrap,
                t.selection,
                t.scroll,
                t.path.clone(),
            )
        };
        ui.text.breakUndoCoalescing();
        ui.layout.replaceTextStorage(&storage);
        ui.text.setEditable(!read_only);
        unsafe {
            ui.text.setTypingAttributes(&self.base_attrs());
        }
        ui.scroll.setRulersVisible(true);
        self.set_wrap(wrap);
        ui.ruler.text_changed();
        let len = storage.length();
        ui.text.setSelectedRange(NSRange::new(
            selection.location.min(len),
            selection.length.min(len - selection.location.min(len)),
        ));
        // The clip view sits under the line-number gutter, so "top-left" is
        // minus the content insets, not (0, 0).
        let insets = ui.scroll.contentView().contentInsets();
        ui.scroll
            .contentView()
            .scrollToPoint(scroll.unwrap_or(NSPoint::new(-insets.left, -insets.top)));
        ui.scroll.reflectScrolledClipView(&ui.scroll.contentView());
        self.apply_highlight();
        if let Some(w) = self.host_window() {
            w.makeFirstResponder(Some(&ui.text));
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.set_title(&name, Some(&path));
        self.rebuild_tab_bar();
        self.vim_reset();
    }

    fn show_placeholder(&self) {
        let ui = self.ui();
        ui.layout.replaceTextStorage(&ui.placeholder);
        ui.text.setEditable(false);
        ui.scroll.setRulersVisible(false);
        self.set_wrap(false);
        let root_name = self
            .ivars()
            .tree
            .borrow()
            .as_ref()
            .map(|t| t.node(ROOT).name.clone());
        if let Some(name) = root_name {
            self.set_title(&name, None);
        }
        self.vim_update_ui();
    }

    fn close_tab(&self, i: usize) {
        let (dirty, name) = {
            let tabs = self.ivars().tabs.borrow();
            let Some(t) = tabs.list.get(i) else { return };
            (
                t.dirty,
                t.path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        };
        if dirty {
            match self.alert(
                &format!("Save changes to {name}?"),
                "Your changes will be lost if you don't save them.",
                &["Save", "Cancel", "Don't Save"],
            ) {
                0 => {
                    if !self.save_tab(i) {
                        return;
                    }
                }
                1 => return,
                _ => {}
            }
        }
        self.remove_tab(i);
    }

    /// Drop a tab without asking; switch to a neighbour if it was current.
    fn remove_tab(&self, i: usize) {
        let next = {
            let mut tabs = self.ivars().tabs.borrow_mut();
            if i >= tabs.list.len() {
                return;
            }
            let was_current = tabs.current == Some(i);
            tabs.list.remove(i);
            match tabs.current {
                Some(c) if c > i => tabs.current = Some(c - 1),
                Some(c) if c == i => tabs.current = None,
                _ => {}
            }
            if was_current && !tabs.list.is_empty() {
                // Most recently used remaining tab, like VS Code.
                tabs.list
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, t)| t.last_used)
                    .map(|(j, _)| j)
            } else {
                None
            }
        };
        match next {
            Some(j) => self.activate_tab(j),
            None => {
                if self.ivars().tabs.borrow().current.is_none() {
                    self.show_placeholder();
                }
                self.rebuild_tab_bar();
            }
        }
        self.update_edited_dot();
    }

    fn cycle_tab(&self, step: isize) {
        let next = {
            let tabs = self.ivars().tabs.borrow();
            let n = tabs.list.len() as isize;
            match tabs.current {
                Some(c) if n > 1 => Some(((c as isize + step).rem_euclid(n)) as usize),
                _ => None,
            }
        };
        if let Some(i) = next {
            self.activate_tab(i);
        }
    }

    fn save_tab(&self, i: usize) -> bool {
        let (path, text, read_only) = {
            let tabs = self.ivars().tabs.borrow();
            let Some(t) = tabs.list.get(i) else { return false };
            (t.path.clone(), t.storage.string().to_string(), t.read_only)
        };
        if read_only {
            self.alert(
                "This file is open read-only",
                "It is binary, not UTF-8, or too large to edit here.",
                &["OK"],
            );
            return false;
        }
        if let Err(e) = fs::write(&path, text) {
            self.alert(
                &format!("Couldn't save {}", path.display()),
                &e.to_string(),
                &["OK"],
            );
            return false;
        }
        {
            let mut tabs = self.ivars().tabs.borrow_mut();
            let t = &mut tabs.list[i];
            t.dirty = false;
            t.preview = false;
            t.mtime = mtime(&path);
        }
        self.rebuild_tab_bar();
        self.git_after_save();
        true
    }

    fn rebuild_tab_bar(&self) {
        let ui = self.ui();
        let mtm = self.mtm();
        for v in ui.tab_bar.arrangedSubviews().iter() {
            ui.tab_bar.removeView(&v);
        }
        let tabs = self.ivars().tabs.borrow();
        let base = NSFont::systemFontOfSize(12.0);
        let italic = NSFontManager::sharedFontManager(mtm)
            .convertFont_toHaveTrait(&base, NSFontTraitMask::ItalicFontMask);
        for (i, t) in tabs.list.iter().enumerate() {
            let current = tabs.current == Some(i);
            let name = t
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let title = if t.dirty { format!("● {name}") } else { name };
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&title),
                    Some(self),
                    Some(sel!(tabClicked:)),
                    mtm,
                )
            };
            button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
            button.setButtonType(NSButtonType::PushOnPushOff);
            button.setState(if current {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            button.setTag(i as isize);
            button.setFont(Some(if t.preview { &italic } else { &base }));
            button.setToolTip(Some(&NSString::from_str(&t.path.to_string_lossy())));
            button.setRefusesFirstResponder(true);
            let close = unsafe {
                NSButton::buttonWithTitle_target_action(
                    ns_string!("×"),
                    Some(self),
                    Some(sel!(tabCloseClicked:)),
                    mtm,
                )
            };
            close.setBordered(false);
            close.setTag(i as isize);
            close.setFont(Some(&NSFont::systemFontOfSize(13.0)));
            close.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            close.setRefusesFirstResponder(true);
            close.setToolTip(Some(ns_string!("Close (⌘W)")));
            ui.tab_bar.addView_inGravity(&button, NSStackViewGravity::Leading);
            ui.tab_bar.addView_inGravity(&close, NSStackViewGravity::Leading);
            ui.tab_bar.setCustomSpacing_afterView(6.0, &close);
            // Let long names shrink before the strip overflows.
            button.setContentCompressionResistancePriority_forOrientation(
                250.0,
                NSLayoutConstraintOrientation::Horizontal,
            );
            if let Some(cell) = button.cell() {
                cell.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
            }
        }
        drop(tabs);
        self.update_edited_dot();
    }

    fn update_edited_dot(&self) {
        let dirty = self.ivars().tabs.borrow().list.iter().any(|t| t.dirty);
        match &self.ui().window {
            Some(w) => w.setDocumentEdited(dirty),
            None => self.emit(ViewerEvent::Title),
        }
    }

    /// Ask about unsaved tabs. Returns false if the user cancelled.
    pub fn confirm_discard_all(&self) -> bool {
        let dirty: Vec<usize> = {
            let tabs = self.ivars().tabs.borrow();
            tabs.list
                .iter()
                .enumerate()
                .filter(|(_, t)| t.dirty)
                .map(|(i, _)| i)
                .collect()
        };
        if dirty.is_empty() {
            return true;
        }
        let msg = if dirty.len() == 1 {
            "You have unsaved changes in 1 file.".to_string()
        } else {
            format!("You have unsaved changes in {} files.", dirty.len())
        };
        match self.alert(
            &msg,
            "Save them before continuing?",
            &["Save All", "Cancel", "Don't Save"],
        ) {
            0 => dirty.into_iter().all(|i| self.save_tab(i)),
            1 => false,
            _ => {
                // Discarding: mark clean so nothing asks again.
                for t in self.ivars().tabs.borrow_mut().list.iter_mut() {
                    t.dirty = false;
                }
                true
            }
        }
    }

    // ---------------------------------------------------------- highlighting

    fn highlight_tab(&self, i: usize) {
        let (lang, storage) = {
            let tabs = self.ivars().tabs.borrow();
            let Some(t) = tabs.list.get(i) else { return };
            let Some(lang) = t.lang else { return };
            (lang, t.storage.clone())
        };
        let text = storage.string().to_string();
        let spans = highlight::highlight(lang, &text);
        if let Some(t) = self.ivars().tabs.borrow_mut().list.get_mut(i) {
            t.spans = spans;
        }
    }

    /// Paint the current tab's spans as temporary attributes, which recolour
    /// without re-laying out text and never touch the undo stack.
    fn apply_highlight(&self) {
        let ui = self.ui();
        let tabs = self.ivars().tabs.borrow();
        let Some(i) = tabs.current else { return };
        let t = &tabs.list[i];
        let len = t.storage.length();
        let key = unsafe { NSForegroundColorAttributeName };
        ui.layout
            .removeTemporaryAttribute_forCharacterRange(key, NSRange::new(0, len));
        for s in &t.spans {
            let (start, slen) = (s.start as usize, s.len as usize);
            if start + slen > len {
                break;
            }
            let color = &ui.colors[s.color as usize];
            unsafe {
                ui.layout
                    .addTemporaryAttribute_value_forCharacterRange(key, color, NSRange::new(start, slen))
            };
        }
    }

    // --------------------------------------------------------------- view

    fn set_wrap(&self, wrap: bool) {
        let ui = self.ui();
        let insets = ui.scroll.contentView().contentInsets();
        let width = ui.scroll.contentSize().width - insets.left - insets.right;
        if wrap {
            ui.scroll.setHasHorizontalScroller(false);
            ui.text.setHorizontallyResizable(false);
            ui.container.setContainerSize(NSSize::new(width, f64::MAX));
            ui.container.setWidthTracksTextView(true);
            ui.text
                .setFrameSize(NSSize::new(width, ui.text.frame().size.height));
        } else {
            ui.scroll.setHasHorizontalScroller(true);
            ui.container.setWidthTracksTextView(false);
            ui.container.setContainerSize(NSSize::new(f64::MAX, f64::MAX));
            ui.text.setHorizontallyResizable(true);
        }
        ui.ruler.text_changed();
    }

    fn set_font_size(&self, size: f64) {
        let size = size.clamp(8.0, 32.0);
        self.ivars().font_size.set(size);
        NSUserDefaults::standardUserDefaults().setDouble_forKey(size, ns_string!("fontSize"));
        let ui = self.ui();
        let attrs = self.base_attrs();
        let mut storages: Vec<Retained<NSTextStorage>> = self
            .ivars()
            .tabs
            .borrow()
            .list
            .iter()
            .map(|t| t.storage.clone())
            .collect();
        storages.push(ui.placeholder.clone());
        for s in storages {
            let len = s.length();
            s.beginEditing();
            unsafe {
                s.setAttributes_range(Some(&attrs), NSRange::new(0, len));
            }
            s.endEditing();
        }
        unsafe {
            ui.text.setTypingAttributes(&attrs);
        }
        ui.ruler.set_metrics(size, self.line_spacing());
        self.vim_update_ui();
        self.apply_highlight();
    }

    // ---------------------------------------------------------- quick open

    fn show_quick_open(&self) {
        let ui = self.ui();
        let Some(root) = self
            .ivars()
            .tree
            .borrow()
            .as_ref()
            .map(|t| t.root().to_path_buf())
        else {
            self.choose_folder();
            return;
        };
        {
            let mut q = self.ivars().quick.borrow_mut();
            let stale = q.built.is_none_or(|b| b.elapsed().as_secs() > 10);
            if stale {
                q.files = fuzzy::list_files(&root);
                q.built = Some(Instant::now());
            }
        }
        let Some(window) = self.host_window() else { return };
        let frame = window.convertRectToScreen(ui.root.convertRect_toView(ui.root.bounds(), None));
        let pw = ui.panel.frame().size.width;
        ui.panel.setFrameTopLeftPoint(NSPoint::new(
            frame.origin.x + (frame.size.width - pw) / 2.0,
            frame.origin.y + frame.size.height - 60.0,
        ));
        ui.field.setStringValue(ns_string!(""));
        self.quick_search();
        ui.panel.makeKeyAndOrderFront(None);
        ui.panel.makeFirstResponder(Some(&ui.field));
    }

    fn quick_search(&self) {
        let ui = self.ui();
        let query = ui.field.stringValue().to_string();
        {
            let mut q = self.ivars().quick.borrow_mut();
            let mut hits = Vec::new();
            if query.trim().is_empty() {
                // Recent files first, then everything else in walk order.
                for r in &q.recent {
                    if let Some(i) = q.files.iter().position(|f| f == r) {
                        hits.push(i);
                    }
                }
                for i in 0..q.files.len().min(60) {
                    if !hits.contains(&i) {
                        hits.push(i);
                    }
                }
            } else {
                hits = fuzzy::rank(&q.files, &query, 60);
            }
            q.hits = hits;
        }
        ui.results.reloadData();
        if !self.ivars().quick.borrow().hits.is_empty() {
            ui.results
                .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(0), false);
            ui.results.scrollRowToVisible(0);
        }
    }

    fn quick_open_selected(&self) {
        let ui = self.ui();
        let row = ui.results.selectedRow();
        let path = {
            let q = self.ivars().quick.borrow();
            let tree = self.ivars().tree.borrow();
            let (Some(t), true) = (tree.as_ref(), row >= 0) else {
                return;
            };
            let Some(rel) = q.hits.get(row as usize).and_then(|&i| q.files.get(i)) else {
                return;
            };
            t.root().join(rel)
        };
        ui.panel.orderOut(None);
        self.open_file(&path, true);
        self.reveal(&path);
    }

    fn remember_recent(&self, path: &Path) {
        let rel = {
            let tree = self.ivars().tree.borrow();
            let Some(t) = tree.as_ref() else { return };
            match path.strip_prefix(t.root()) {
                Ok(r) => r.to_string_lossy().into_owned(),
                Err(_) => return,
            }
        };
        let mut q = self.ivars().quick.borrow_mut();
        q.recent.retain(|r| *r != rel);
        q.recent.insert(0, rel);
        q.recent.truncate(20);
    }

    // ------------------------------------------------------------ editing

    fn go_to_line(&self) {
        if self.ivars().tabs.borrow().current.is_none() {
            return;
        }
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(ns_string!("Go to Line"));
        alert.addButtonWithTitle(ns_string!("Go"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        let input = NSTextField::new(self.mtm());
        input.setFrame(rect(0.0, 0.0, 200.0, 24.0));
        input.setPlaceholderString(Some(ns_string!("line[:column]")));
        alert.setAccessoryView(Some(&input));
        alert.window().setInitialFirstResponder(Some(&input));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return;
        }
        let entry = input.stringValue().to_string();
        let mut parts = entry.trim().split(':');
        let Some(line) = parts.next().and_then(|s| s.trim().parse::<usize>().ok()) else {
            return;
        };
        let col = parts
            .next()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(1);
        self.go_to(line, col);
    }

    /// Put the caret at a 1-based line and column of the current file.
    fn go_to(&self, line: usize, col: usize) {
        let ui = self.ui();
        let target = {
            let starts = ui.ruler.line_starts(&ui.text);
            let li = line.clamp(1, starts.len()) - 1;
            let start = starts[li] as usize;
            let end = starts
                .get(li + 1)
                .map_or(ui.text.string().length(), |&e| e as usize - 1);
            (start + col.saturating_sub(1)).min(end)
        };
        let r = NSRange::new(target, 0);
        ui.text.setSelectedRange(r);
        ui.text.scrollRangeToVisible(r);
        // scrollRangeToVisible doesn't know the line-number gutter covers the
        // clip view's left edge (origin x is -inset at rest), so near the
        // start of a line it scrolls the first characters under the gutter.
        // Put x back at rest when the column fits in the visible width.
        let clip = ui.scroll.contentView();
        let insets = clip.contentInsets();
        let mut origin = clip.bounds().origin;
        let char_w = unsafe {
            NSString::from_str("M").sizeWithAttributes(Some(&attrs(&[(NSFontAttributeName, &*self.font())])))
        }
        .width;
        let visible = clip.bounds().size.width - insets.left;
        if (col as f64) * char_w < visible - 4.0 * char_w {
            origin.x = -insets.left;
            clip.scrollToPoint(origin);
            ui.scroll.reflectScrolledClipView(&clip);
        }
        if let Some(w) = self.host_window() {
            w.makeFirstResponder(Some(&ui.text));
        }
    }

    fn toggle_comment(&self) {
        let ui = self.ui();
        let lang = {
            let tabs = self.ivars().tabs.borrow();
            match tabs.current {
                Some(i) if !tabs.list[i].read_only => tabs.list[i].lang,
                _ => return,
            }
        };
        let Some(prefix) = comment_prefix(lang) else {
            return;
        };
        let text = ui.text.string();
        let lines_range = text.lineRangeForRange(selected_range(&ui.text));
        let block = text.substringWithRange(lines_range).to_string();
        let Some(out) = edit::toggle_line_comment(&block, prefix) else {
            return;
        };
        let new = NSString::from_str(&out);
        unsafe { ui.text.insertText_replacementRange(&new, lines_range) };
        ui.text
            .setSelectedRange(NSRange::new(lines_range.location, new.length()));
    }

    // ------------------------------------------------------------ dialogs

    /// Modal alert; returns the index of the clicked button.
    fn alert(&self, message: &str, info: &str, buttons: &[&str]) -> usize {
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(info));
        for b in buttons {
            alert.addButtonWithTitle(&NSString::from_str(b));
        }
        (alert.runModal() - NSAlertFirstButtonReturn).max(0) as usize
    }
}
