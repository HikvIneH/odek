//! `odek --term [dir]`: the app delegate for the terminal. Normal launches
//! open the workspace window (see window.rs); the selftest snapshot mode
//! uses plain one-shell windows.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationDelegate, NSApplicationTerminateReply,
    NSBackingStoreType, NSControlStateValueOn, NSEventModifierFlags, NSMenu, NSMenuItem,
    NSRequestUserAttentionType, NSTextFinderAction, NSWindow, NSWindowDelegate, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSNotification, NSPoint, NSRect, NSSize, NSString, NSURL, NSUserDefaults};

use super::view::{TermView, ViewEvent};
use super::window::Workbench;

const APP_NAME: &str = "Odek";

thread_local! {
    static INSTANCE: OnceCell<Retained<TermApp>> = const { OnceCell::new() };
}

fn instance() -> Option<Retained<TermApp>> {
    INSTANCE.with(|i| i.get().cloned())
}

struct Pane {
    window: Retained<NSWindow>,
    view: Retained<TermView>,
}

pub struct Ivars {
    start_dir: PathBuf,
    /// A folder named on the command line: opened as a new tab.
    open_dir: Option<PathBuf>,
    panes: RefCell<Vec<Pane>>,
    bench: OnceCell<Rc<Workbench>>,
    /// Paths that arrived (Finder, `open -a`) before the window existed.
    pending: RefCell<Vec<PathBuf>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct TermApp;

    unsafe impl NSObjectProtocol for TermApp {}

    unsafe impl NSApplicationDelegate for TermApp {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            let app = NSApplication::sharedApplication(self.mtm());
            app.setMainMenu(Some(&self.build_menu()));
            #[cfg(feature = "selftest")]
            if snap::active() {
                return snap::start(self);
            }
            let bench = Workbench::new(ProtocolObject::from_ref(self), self.mtm());
            bench.start(self.ivars().open_dir.clone(), true);
            for path in self.ivars().pending.take() {
                bench.open_path(&path);
            }
            let _ = self.ivars().bench.set(bench);
            super::notify::setup();
            tick();
            app.activate();
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_last_window(&self, _app: &NSApplication) -> bool {
            true
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            for url in urls.iter() {
                let Some(path) = url.to_file_path() else { continue };
                match self.ivars().bench.get() {
                    Some(b) => b.open_path(&path),
                    None => self.ivars().pending.borrow_mut().push(path.to_path_buf()),
                }
            }
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _n: &NSNotification) {
            self.with_bench(|b| b.app_activated());
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            if self.confirm_running("Quit Odek?") {
                NSApplicationTerminateReply::TerminateNow
            } else {
                NSApplicationTerminateReply::TerminateCancel
            }
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            if let Some(b) = self.ivars().bench.get() {
                b.shutdown_all();
            }
        }
    }

    unsafe impl NSWindowDelegate for TermApp {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, sender: &NSWindow) -> bool {
            match self.ivars().bench.get() {
                Some(b) if std::ptr::eq(&*b.window, sender) => self.confirm_running("Close the window?"),
                _ => true,
            }
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _n: &NSNotification) {
            if let Some(b) = self.ivars().bench.get() {
                b.window_became_key();
            }
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, n: &NSNotification) {
            let Some(obj) = n.object() else { return };
            let mut panes = self.ivars().panes.borrow_mut();
            if let Some(i) = panes.iter().position(|p| std::ptr::eq(&*p.window as *const _ as *const AnyObject, &*obj)) {
                let pane = panes.remove(i);
                pane.view.shutdown();
            }
        }
    }

    impl TermApp {
        #[unsafe(method(termSettings:))]
        fn menu_settings(&self, _sender: Option<&AnyObject>) {
            super::settings::show(self.mtm());
        }

        #[unsafe(method(termNewTab:))]
        fn menu_new_tab(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.new_tab());
        }

        #[unsafe(method(termNewGroup:))]
        fn menu_new_group(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.new_group());
        }

        #[unsafe(method(termSplitRight:))]
        fn menu_split_right(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.split(true));
        }

        #[unsafe(method(termSplitDown:))]
        fn menu_split_down(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.split(false));
        }

        #[unsafe(method(termClosePane:))]
        fn menu_close_pane(&self, _sender: Option<&AnyObject>) {
            match self.ivars().bench.get() {
                Some(b) => b.request_close_focused(),
                None => {
                    let app = NSApplication::sharedApplication(self.mtm());
                    if let Some(w) = app.keyWindow() {
                        w.performClose(None);
                    }
                }
            }
        }

        #[unsafe(method(termCloseTab:))]
        fn menu_close_tab(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.request_close_active_tab());
        }

        #[unsafe(method(termRenameTab:))]
        fn menu_rename_tab(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.rename_active_tab());
        }

        #[unsafe(method(termNextTab:))]
        fn menu_next_tab(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.cycle_tab(true));
        }

        #[unsafe(method(termPreviousTab:))]
        fn menu_previous_tab(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.cycle_tab(false));
        }

        #[unsafe(method(termNextPane:))]
        fn menu_next_pane(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.cycle_pane(true));
        }

        #[unsafe(method(termPreviousPane:))]
        fn menu_previous_pane(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.cycle_pane(false));
        }

        #[unsafe(method(termSelectTab:))]
        fn menu_select_tab(&self, sender: Option<&NSMenuItem>) {
            if let Some(item) = sender {
                let n = item.tag() as usize;
                self.with_bench(|b| b.select_index(n));
            }
        }

        #[unsafe(method(termToggleSidebar:))]
        fn menu_toggle_sidebar(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.toggle_sidebar());
        }

        // Reaches here only when a terminal has focus; the viewer handles
        // its own ⌘P and ⇧⌘E first through the responder chain.
        #[unsafe(method(appQuickOpen:))]
        fn menu_quick_open(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.quick_open_here());
        }

        /// ⌘O is one menu item: here (a terminal has focus) it's New Tab in
        /// Folder; the code viewer answers it as Open Folder and renames it.
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            if item.action() == Some(sel!(appOpenFolder:)) {
                item.setTitle(&NSString::from_str("New Tab in Folder…"));
            }
            true
        }

        #[unsafe(method(appOpenFolder:))]
        fn menu_open(&self, _sender: Option<&AnyObject>) {
            let panel = objc2_app_kit::NSOpenPanel::openPanel(self.mtm());
            panel.setCanChooseDirectories(true);
            panel.setCanChooseFiles(true);
            panel.setAllowsMultipleSelection(true);
            panel.setPrompt(Some(&NSString::from_str("New Tab")));
            panel.setMessage(Some(&NSString::from_str(
                "Opens a new terminal tab in the folder. A file opens in the code viewer.",
            )));
            if panel.runModal() == objc2_app_kit::NSModalResponseOK {
                for url in panel.URLs().iter() {
                    if let Some(path) = url.to_file_path() {
                        self.with_bench(|b| b.open_path(&path));
                    }
                }
            }
        }

        #[unsafe(method(termShowFiles:))]
        fn menu_show_files(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.show_files_here());
        }

        #[unsafe(method(termShowShortcuts:))]
        fn menu_show_shortcuts(&self, _sender: Option<&AnyObject>) {
            super::shortcuts::toggle(self.mtm());
        }

        #[unsafe(method(termSearchTabs:))]
        fn menu_search_tabs(&self, _sender: Option<&AnyObject>) {
            self.with_bench(|b| b.search_tabs());
        }

        #[unsafe(method(termNewWindow:))]
        fn menu_new_window(&self, _sender: Option<&AnyObject>) {
            let dir = self.key_view().and_then(|v| v.session_cwd()).unwrap_or_else(|| self.ivars().start_dir.clone());
            self.open_window(&dir, None, true);
        }

        #[unsafe(method(termToggleNotify:))]
        fn menu_toggle_notify(&self, sender: Option<&NSMenuItem>) {
            let on = !super::notify::enabled();
            super::notify::set_enabled(on);
            if let Some(item) = sender {
                item.setState(if on { NSControlStateValueOn } else { 0 });
            }
        }

        #[unsafe(method(termZoomIn:))]
        fn menu_zoom_in(&self, _sender: Option<&AnyObject>) {
            self.zoom(1.0);
        }

        #[unsafe(method(termZoomOut:))]
        fn menu_zoom_out(&self, _sender: Option<&AnyObject>) {
            self.zoom(-1.0);
        }

        #[unsafe(method(termZoomReset:))]
        fn menu_zoom_reset(&self, _sender: Option<&AnyObject>) {
            if let Some(v) = self.key_view() {
                v.set_font_size(super::settings::font_size());
            }
        }
    }
);

impl TermApp {
    pub fn new(open_dir: Option<PathBuf>, mtm: MainThreadMarker) -> Retained<Self> {
        let start_dir = open_dir
            .clone()
            .or_else(|| std::env::var_os("HOME").map(Into::into))
            .unwrap_or_else(|| "/".into());
        let this = Self::alloc(mtm).set_ivars(Ivars {
            start_dir,
            open_dir,
            panes: RefCell::new(Vec::new()),
            bench: OnceCell::new(),
            pending: RefCell::new(Vec::new()),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        INSTANCE.with(|i| {
            let _ = i.set(this.clone());
        });
        this
    }

    fn with_bench(&self, f: impl FnOnce(&Workbench)) {
        if let Some(b) = self.ivars().bench.get() {
            f(b);
        }
    }

    /// The terminal with keyboard focus.
    fn key_view(&self) -> Option<Retained<TermView>> {
        let app = NSApplication::sharedApplication(self.mtm());
        let responder = app.keyWindow()?.firstResponder()?;
        responder.downcast::<TermView>().ok()
    }

    /// True when nothing is running and no file is unsaved, or the user
    /// agrees to end and discard.
    fn confirm_running(&self, question: &str) -> bool {
        let Some(b) = self.ivars().bench.get() else {
            return true;
        };
        if !b.viewer_confirm_discard() {
            return false;
        }
        let running = b.running_everywhere();
        if running.is_empty() {
            return true;
        }
        let mtm = self.mtm();
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(question));
        let mut names = running.clone();
        names.sort();
        names.dedup();
        alert.setInformativeText(&NSString::from_str(&format!(
            "Still running: {}. Closing ends these processes.",
            names.join(", ")
        )));
        alert.addButtonWithTitle(&NSString::from_str("Close"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        alert.runModal() == NSAlertFirstButtonReturn
    }

    fn zoom(&self, delta: f64) {
        if let Some(v) = self.key_view() {
            v.set_font_size(v.font_size() + delta);
        }
    }

    pub fn open_window(&self, dir: &Path, command: Option<&str>, show: bool) -> Retained<TermView> {
        let mtm = self.mtm();
        // 100×30 cells.
        let probe = TermView::new(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 100.0)),
            mtm,
        );
        let (cw, ch) = probe.cell_size();
        probe.shutdown();
        let (px, py) = TermView::padding();
        let size = NSSize::new((100.0 * cw + 2.0 * px).ceil(), (30.0 * ch + 2.0 * py).ceil());
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
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
        window.setTitle(&NSString::from_str(&title_for(dir)));
        window.setTabbingMode(objc2_app_kit::NSWindowTabbingMode::Disallowed);
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.setContentMinSize(NSSize::new(200.0, 80.0));
        // 8-bit buffers, as in the workspace window.
        window.setColorSpace(Some(&objc2_app_kit::NSColorSpace::sRGBColorSpace()));

        let view = TermView::new(frame, mtm);
        window.setContentView(Some(&view));
        window.makeFirstResponder(Some(&view));

        let weak_window = Weak::from_retained(&window);
        view.set_on_event(move |e| {
            let Some(window) = weak_window.load() else { return };
            match e {
                ViewEvent::Title(t) => window.setTitle(&NSString::from_str(&t)),
                ViewEvent::Attention(_) => {
                    let app = NSApplication::sharedApplication(MainThreadMarker::from(&*window));
                    if !app.isActive() {
                        app.requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
                    }
                }
                ViewEvent::Exited(_) => window.close(),
                ViewEvent::Focused => {}
                // Default app for now; an in-app editor can take over later.
                ViewEvent::OpenPath { path, .. } => {
                    if let Some(url) = path
                        .to_str()
                        .map(|p| NSURL::fileURLWithPath(&NSString::from_str(p)))
                    {
                        NSWorkspace::sharedWorkspace().openURL(&url);
                    }
                }
            }
        });
        if let Err(e) = view.start(dir, command) {
            eprintln!("odek: could not start shell: {e}");
        }
        if show {
            window.cascadeTopLeftFromPoint(NSPoint::new(40.0, 40.0));
            window.center();
            window.makeKeyAndOrderFront(None);
        }
        self.ivars().panes.borrow_mut().push(Pane {
            window,
            view: view.clone(),
        });
        view
    }

    fn build_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let cmd = NSEventModifierFlags::Command;
        let opt = NSEventModifierFlags::Option;
        let shift = NSEventModifierFlags::Shift;
        let item = |title: &str, action: Option<Sel>, key: &str, mods: NSEventModifierFlags| {
            let it = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    action,
                    &NSString::from_str(key),
                )
            };
            it.setKeyEquivalentModifierMask(mods);
            it
        };
        let menu = |title: &str, items: Vec<Retained<NSMenuItem>>| {
            let m = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
            for it in items {
                m.addItem(&it);
            }
            let holder = item(title, None, "", cmd);
            holder.setSubmenu(Some(&m));
            holder
        };
        let sep = || NSMenuItem::separatorItem(mtm);
        // Function keys are key equivalents by their AppKit code point.
        let key = |code: u32| char::from_u32(code).unwrap().to_string();
        let ctrl = NSEventModifierFlags::Control;
        // Text finder commands carry their action in the tag; only the code
        // viewer's text view answers them, so they're greyed out in terminals.
        let finder = |title: &str, key: &str, mods, action: NSTextFinderAction| {
            let it = item(title, Some(sel!(performTextFinderAction:)), key, mods);
            it.setTag(action.0);
            it
        };
        let vim = item("Vim Mode", Some(sel!(appToggleVim:)), "v", cmd | opt);
        if NSUserDefaults::standardUserDefaults().boolForKey(&NSString::from_str("vimMode")) {
            vim.setState(NSControlStateValueOn);
        }
        let notify = item(
            "Notify When a Background Tab Needs Attention",
            Some(sel!(termToggleNotify:)),
            "",
            cmd,
        );
        if super::notify::enabled() {
            notify.setState(NSControlStateValueOn);
        }
        let next_file = item("Next File", Some(sel!(appNextTab:)), "\t", ctrl);
        let prev_file = item("Previous File", Some(sel!(appPrevTab:)), "\t", ctrl | shift);
        let bar = NSMenu::new(mtm);
        for m in [
            menu(
                APP_NAME,
                vec![
                    item(
                        &format!("About {APP_NAME}"),
                        Some(sel!(orderFrontStandardAboutPanel:)),
                        "",
                        cmd,
                    ),
                    sep(),
                    item("Settings…", Some(sel!(termSettings:)), ",", cmd),
                    sep(),
                    item(&format!("Hide {APP_NAME}"), Some(sel!(hide:)), "h", cmd),
                    item("Hide Others", Some(sel!(hideOtherApplications:)), "h", cmd | opt),
                    sep(),
                    item(&format!("Quit {APP_NAME}"), Some(sel!(terminate:)), "q", cmd),
                ],
            ),
            menu(
                "Shell",
                vec![
                    item("New Tab", Some(sel!(termNewTab:)), "t", cmd),
                    item("New Tab in Folder…", Some(sel!(appOpenFolder:)), "o", cmd),
                    item("New Group…", Some(sel!(termNewGroup:)), "n", cmd | shift),
                    sep(),
                    item("Split Right", Some(sel!(termSplitRight:)), "d", cmd),
                    item("Split Down", Some(sel!(termSplitDown:)), "d", cmd | shift),
                    sep(),
                    item("Rename Tab…", Some(sel!(termRenameTab:)), "r", cmd | shift),
                    sep(),
                    item("Close Pane", Some(sel!(termClosePane:)), "w", cmd),
                    item("Close Tab", Some(sel!(termCloseTab:)), "w", cmd | shift),
                ],
            ),
            menu(
                "File",
                vec![
                    item("Quick Open…", Some(sel!(appQuickOpen:)), "p", cmd),
                    item("Show Files", Some(sel!(termShowFiles:)), "e", cmd | shift),
                    sep(),
                    item("Save", Some(sel!(appSave:)), "s", cmd),
                    item("Reveal in Finder", Some(sel!(appRevealInFinder:)), "r", cmd | opt),
                ],
            ),
            menu(
                "Edit",
                vec![
                    item("Undo", Some(sel!(undo:)), "z", cmd),
                    item("Redo", Some(sel!(redo:)), "z", cmd | shift),
                    sep(),
                    item("Cut", Some(sel!(cut:)), "x", cmd),
                    item("Copy", Some(sel!(copy:)), "c", cmd),
                    item("Paste", Some(sel!(paste:)), "v", cmd),
                    item("Select All", Some(sel!(selectAll:)), "a", cmd),
                    sep(),
                    item("Toggle Line Comment", Some(sel!(appToggleComment:)), "/", cmd),
                    item("Go to Line…", Some(sel!(appGoToLine:)), "g", ctrl),
                    sep(),
                    item("Find…", Some(sel!(termFind:)), "f", cmd),
                    finder(
                        "Replace…",
                        "f",
                        cmd | opt,
                        NSTextFinderAction::ShowReplaceInterface,
                    ),
                    item("Find Next", Some(sel!(termFindNext:)), "g", cmd),
                    item("Find Previous", Some(sel!(termFindPrevious:)), "g", cmd | shift),
                    finder(
                        "Use Selection for Find",
                        "e",
                        cmd,
                        NSTextFinderAction::SetSearchString,
                    ),
                    sep(),
                    item("Clear to Previous Mark", Some(sel!(termClearToMark:)), "l", cmd),
                    item("Clear Screen", Some(sel!(termClearScreen:)), "l", cmd | ctrl),
                    item("Clear to Start", Some(sel!(clearScrollback:)), "k", cmd),
                    item(
                        "Clear Scrollback",
                        Some(sel!(termClearScrollbackOnly:)),
                        "k",
                        cmd | opt,
                    ),
                    sep(),
                    item(
                        "Emoji & Symbols",
                        Some(sel!(orderFrontCharacterPalette:)),
                        "",
                        cmd,
                    ),
                ],
            ),
            menu(
                "View",
                vec![
                    item("Toggle Sidebar", Some(sel!(termToggleSidebar:)), "b", cmd),
                    item("Search Tabs…", Some(sel!(termSearchTabs:)), "f", cmd | shift),
                    sep(),
                    item("Previous Mark", Some(sel!(termPreviousMark:)), &key(0xF700), cmd),
                    item("Next Mark", Some(sel!(termNextMark:)), &key(0xF701), cmd),
                    item("Scroll to Top", Some(sel!(termScrollToTop:)), &key(0xF729), cmd),
                    item(
                        "Scroll to Bottom",
                        Some(sel!(termScrollToBottom:)),
                        &key(0xF72B),
                        cmd,
                    ),
                    item("Page Up", Some(sel!(termPageUp:)), &key(0xF72C), cmd),
                    item("Page Down", Some(sel!(termPageDown:)), &key(0xF72D), cmd),
                    item("Line Up", Some(sel!(termLineUp:)), &key(0xF72C), cmd | opt),
                    item("Line Down", Some(sel!(termLineDown:)), &key(0xF72D), cmd | opt),
                    sep(),
                    item("Toggle Word Wrap", Some(sel!(appToggleWrap:)), "z", opt),
                    vim,
                    notify,
                    sep(),
                    item("Bigger", Some(sel!(termZoomIn:)), "=", cmd),
                    item("Smaller", Some(sel!(termZoomOut:)), "-", cmd),
                    item("Actual Size", Some(sel!(termZoomReset:)), "0", cmd),
                ],
            ),
            menu("Window", {
                let mut items = vec![
                    item("Minimize", Some(sel!(performMiniaturize:)), "m", cmd),
                    sep(),
                    item("Next Tab", Some(sel!(termNextTab:)), "]", cmd | shift),
                    item("Previous Tab", Some(sel!(termPreviousTab:)), "[", cmd | shift),
                    item("Next Pane", Some(sel!(termNextPane:)), "]", cmd),
                    item("Previous Pane", Some(sel!(termPreviousPane:)), "[", cmd),
                    next_file,
                    prev_file,
                    sep(),
                ];
                for n in 1..=9 {
                    let title = if n == 9 {
                        "Last Tab".to_string()
                    } else {
                        format!("Tab {n}")
                    };
                    let it = item(&title, Some(sel!(termSelectTab:)), &n.to_string(), cmd);
                    it.setTag(n);
                    items.push(it);
                }
                items
            }),
            // ⌘/ lives on Edit ▸ Toggle Line Comment: a terminal answers that
            // item by showing this panel (and renames it), the code viewer by
            // commenting. Two items can't share a key equivalent.
            menu(
                "Help",
                vec![item(
                    "Keyboard Shortcuts",
                    Some(sel!(termShowShortcuts:)),
                    "",
                    cmd,
                )],
            ),
        ] {
            bar.addItem(&m);
        }
        bar
    }
}

fn title_for(dir: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    }
}

/// A notification was clicked: bring its pane forward (or just activate).
pub fn reveal_pane(pane: Option<super::workspace::Id>) {
    if let Some(app) = instance() {
        NSApplication::sharedApplication(app.mtm()).activate();
        if let Some(id) = pane {
            app.with_bench(|b| b.reveal_pane(id));
        }
    }
}

/// Settings changed: window-level appearance (opacity, blur).
pub fn appearance_changed() {
    if let Some(app) = instance()
        && let Some(b) = app.ivars().bench.get()
    {
        b.apply_appearance();
    }
}

/// Every second, on the main thread: refresh folders and status, save.
fn tick() {
    if let Some(app) = instance()
        && let Some(b) = app.ivars().bench.get()
    {
        b.tick();
    }
    let when = dispatch2::DispatchTime::try_from(std::time::Duration::from_secs(1)).unwrap();
    let _ = dispatch2::DispatchQueue::main().after(when, tick);
}

pub fn run(open_dir: Option<PathBuf>) {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    #[cfg(feature = "selftest")]
    let policy = if snap::active() {
        objc2_app_kit::NSApplicationActivationPolicy::Accessory
    } else {
        objc2_app_kit::NSApplicationActivationPolicy::Regular
    };
    #[cfg(not(feature = "selftest"))]
    let policy = objc2_app_kit::NSApplicationActivationPolicy::Regular;
    app.setActivationPolicy(policy);
    let delegate = TermApp::new(open_dir, mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}

/// Off-screen scripted run for testing rendering without screen capture:
///
/// ODEK_TERM_SNAP=<out dir> ODEK_TERM_CMD='<command>' \
/// ODEK_TERM_STEPS=$'wait 3\nkeys hello\\r\nsnap name\nquit' odek --term  (one step per line)
#[cfg(feature = "selftest")]
mod snap {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::time::Duration;

    use dispatch2::{DispatchQueue, DispatchTime};
    use objc2::runtime::ProtocolObject;
    use objc2::{DefinedClass, MainThreadOnly};

    use super::super::window::Workbench;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSBitmapImageFileType, NSView};
    use objc2_foundation::NSDictionary;

    use super::super::view::TermView;
    use super::TermApp;

    /// The view, the steps left (last first) and the output folder.
    type Script = (Retained<TermView>, Vec<String>, PathBuf);

    thread_local! {
        static STATE: RefCell<Option<Script>> = const { RefCell::new(None) };
    }

    pub fn active() -> bool {
        std::env::var_os("ODEK_TERM_SNAP").is_some()
    }

    pub fn start(app: &TermApp) {
        let out = PathBuf::from(std::env::var_os("ODEK_TERM_SNAP").unwrap());
        let _ = std::fs::create_dir_all(&out);
        let cmd = std::env::var("ODEK_TERM_CMD").ok();
        let steps: Vec<String> = std::env::var("ODEK_TERM_STEPS")
            .unwrap_or_else(|_| "wait 2\nsnap screen\nquit".into())
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .rev()
            .collect();
        let dir = app.ivars().start_dir.clone();
        // ODEK_TERM_WS=1: drive the workspace window instead of a plain one.
        let view = if std::env::var_os("ODEK_TERM_WS").is_some() {
            let bench = Workbench::new(ProtocolObject::from_ref(app), app.mtm());
            bench
                .window
                .setContentSize(objc2_foundation::NSSize::new(1200.0, 700.0));
            bench.start(Some(dir), false);
            let view = bench.focused_term().unwrap();
            let _ = app.ivars().bench.set(bench);
            view
        } else {
            app.open_window(&dir, cmd.as_deref(), false)
        };
        println!("SNAP pid={}", std::process::id());
        STATE.with(|s| *s.borrow_mut() = Some((view, steps, out)));
        next();
    }

    fn app_mtm() -> objc2::MainThreadMarker {
        objc2::MainThreadMarker::new().unwrap()
    }

    fn after(secs: f64) {
        let when = DispatchTime::try_from(Duration::from_secs_f64(secs)).unwrap();
        let _ = DispatchQueue::main().after(when, next);
    }

    fn next() {
        let step = STATE.with(|s| s.borrow_mut().as_mut().and_then(|(_, steps, _)| steps.pop()));
        let Some(step) = step else { return };
        let (view, out) = STATE.with(|s| {
            let s = s.borrow();
            let (v, _, o) = s.as_ref().unwrap();
            (v.clone(), o.clone())
        });
        let bench = super::instance().and_then(|a| a.ivars().bench.get().cloned());
        // In the workspace, steps act on the focused pane.
        let view = bench.as_ref().and_then(|b| b.focused_term()).unwrap_or(view);
        let (verb, arg) = step.split_once(' ').unwrap_or((&step, ""));
        match verb {
            "wait" => return after(arg.parse().unwrap_or(1.0)),
            "keys" => {
                let text = arg
                    .replace("\\r", "\r")
                    .replace("\\e", "\x1b")
                    .replace("\\t", "\t");
                view.write(text.as_bytes());
            }
            "line" => view.submit(arg),
            "insert" => view.commit_text(arg),
            "newtab" => bench.iter().for_each(|b| b.new_tab()),
            "split" => bench.iter().for_each(|b| b.split(arg != "down")),
            "rename" => bench.iter().for_each(|b| b.name_active_tab(arg)),
            "renamegroup" => bench.iter().for_each(|b| b.name_active_group(arg)),
            "group" => bench.iter().for_each(|b| b.move_active_to_new_group(arg)),
            "nexttab" => bench.iter().for_each(|b| b.cycle_tab(true)),
            "open" => {
                // open <path>[:line[:col]]
                let mut parts = arg.splitn(3, ':');
                let path = PathBuf::from(parts.next().unwrap_or(""));
                let line = parts.next().and_then(|v| v.parse().ok());
                let col = parts.next().and_then(|v| v.parse().ok());
                bench.iter().for_each(|b| b.open_in_viewer(&path, line, col));
            }
            "quickopen" => bench.iter().for_each(|b| b.quick_open_here()),
            "closepane" => bench.iter().for_each(|b| b.request_close_focused()),
            "viewerscroll" => bench
                .iter()
                .for_each(|b| println!("SNAP viewer {}", b.viewer_debug())),
            "files" => bench.iter().for_each(|b| b.show_files_here()),
            "focusterm" => bench.iter().for_each(|b| b.cycle_pane(false)),
            "frames" => {
                fn dump(v: &objc2_app_kit::NSView, depth: usize) {
                    let f = v.frame();
                    println!(
                        "SNAP {}{} ({:.0},{:.0} {:.0}x{:.0}){}",
                        "  ".repeat(depth),
                        v.class().name().to_str().unwrap_or("?"),
                        f.origin.x,
                        f.origin.y,
                        f.size.width,
                        f.size.height,
                        if v.isHidden() { " hidden" } else { "" }
                    );
                    if depth < 7 {
                        for sub in v.subviews().iter() {
                            dump(&sub, depth + 1);
                        }
                    }
                }
                if let Some(content) = bench.as_ref().and_then(|b| b.window.contentView()) {
                    dump(&content, 0);
                }
            }
            "snappane" => {
                if let Some(container) = unsafe { view.superview() } {
                    snapshot(&container, &out.join(format!("{arg}.png")));
                }
            }
            "snapws" => {
                bench.iter().for_each(|b| b.tick());
                if let Some(content) = bench.as_ref().and_then(|b| b.window.contentView()) {
                    snapshot(&content, &out.join(format!("{arg}.png")));
                    println!("SNAP {arg}: footprint {:.1} MB", footprint_mb());
                }
            }
            "mark" => view.mark_text(arg, objc2_foundation::NSRange::new(arg.encode_utf16().count(), 0)),
            "resize" => {
                if let Some((w, h)) = arg.split_once('x') {
                    let (cw, ch) = view.cell_size();
                    let (px, py) = TermView::padding();
                    let w = w.parse::<f64>().unwrap_or(80.0) * cw + 2.0 * px;
                    let h = h.parse::<f64>().unwrap_or(24.0) * ch + 2.0 * py;
                    if let Some(win) = view.window() {
                        win.setContentSize(objc2_foundation::NSSize::new(w.ceil(), h.ceil()));
                    }
                }
            }
            "hover" | "linkat" => {
                let mut n = arg.split_whitespace().filter_map(|v| v.parse::<usize>().ok());
                if let (Some(c), Some(r)) = (n.next(), n.next()) {
                    println!("SNAP {verb} {c} {r}: {}", view.probe_link(c, r, verb == "hover"));
                }
            }
            "snap" => {
                snapshot(&view, &out.join(format!("{arg}.png")));
                std::fs::write(out.join(format!("{arg}.txt")), view.screen_text()).ok();
                println!(
                    "SNAP {arg}: footprint {:.1} MB, terminal {:.2} MB",
                    footprint_mb(),
                    view.mem_bytes() as f64 / 1048576.0
                );
            }
            "cmd" => {
                // cmd <selector name, no colon>: invoke a menu action on the view.
                let sel = objc2::runtime::Sel::register(&std::ffi::CString::new(format!("{arg}:")).unwrap());
                let _: () = unsafe {
                    objc2::msg_send![&*view, performSelector: sel, withObject: std::ptr::null::<objc2::runtime::AnyObject>()]
                };
            }
            "dump" => {
                std::fs::write(out.join(format!("{arg}.txt")), view.all_text()).ok();
            }
            "settings" => super::super::settings::show(app_mtm()),
            // menukey <char> [ctrl|opt|shift|cmd …]: a real key event through the menus.
            "menukey" => {
                let mut parts = arg.split_whitespace();
                let ch = parts.next().unwrap_or("");
                let mut mods = objc2_app_kit::NSEventModifierFlags::empty();
                for m in parts {
                    mods |= match m {
                        "ctrl" => objc2_app_kit::NSEventModifierFlags::Control,
                        "opt" => objc2_app_kit::NSEventModifierFlags::Option,
                        "shift" => objc2_app_kit::NSEventModifierFlags::Shift,
                        _ => objc2_app_kit::NSEventModifierFlags::Command,
                    };
                }
                let chars = objc2_foundation::NSString::from_str(ch);
                let window = view.window();
                let event = objc2_app_kit::NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                    objc2_app_kit::NSEventType::KeyDown,
                    objc2_foundation::NSPoint::new(0.0, 0.0),
                    mods,
                    0.0,
                    window.as_ref().map_or(0, |w| w.windowNumber()),
                    None,
                    &chars,
                    &chars,
                    false,
                    0,
                );
                if let Some(event) = event {
                    let handled = objc2_app_kit::NSApplication::sharedApplication(app_mtm())
                        .mainMenu()
                        .is_some_and(|m| m.performKeyEquivalent(&event));
                    println!(
                        "SNAP menukey {arg}: handled {handled}, shortcuts panel {}",
                        super::super::shortcuts::is_visible()
                    );
                }
            }
            "snapshortcuts" => {
                if let Some(content) = super::super::shortcuts::content_view() {
                    snapshot(&content, &out.join(format!("{arg}.png")));
                }
            }
            "setting" => {
                let (name, value) = arg.split_once(' ').unwrap_or((arg, ""));
                super::super::settings::set_raw(name, value);
            }
            "snapwin" => {
                if let Some(content) = super::super::settings::content_view() {
                    snapshot(&content, &out.join(format!("{arg}.png")));
                }
            }
            "find" => super::super::findbar::open_with(&view, arg),
            "findnext" => super::super::findbar::step(&view, 1),
            "findprev" => super::super::findbar::step(&view, -1),
            "findclose" => super::super::findbar::close(&view),
            "mem" => println!(
                "SNAP mem: footprint {:.1} MB, terminal {:.2} MB",
                footprint_mb(),
                view.mem_bytes() as f64 / 1048576.0
            ),
            "quit" => {
                // No confirmation dialogs in a scripted run: end everything.
                view.shutdown();
                bench.iter().for_each(|b| b.shutdown_all());
                std::process::exit(0);
            }
            other => eprintln!("SNAP unknown step {other}"),
        }
        after(0.05);
    }

    /// Same number Activity Monitor shows as "Memory".
    fn footprint_mb() -> f64 {
        let mut info: libc::rusage_info_v0 = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as i32,
                libc::RUSAGE_INFO_V0,
                (&raw mut info).cast(),
            )
        } == 0;
        if ok {
            info.ri_phys_footprint as f64 / 1048576.0
        } else {
            0.0
        }
    }

    fn snapshot(view: &NSView, path: &std::path::Path) {
        let bounds = view.bounds();
        let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else {
            return;
        };
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let props = NSDictionary::new();
        if let Some(data) =
            unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        {
            let _ = std::fs::write(path, data.to_vec());
        }
    }
}
