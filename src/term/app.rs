//! `odek --term [dir]`: terminal windows, one shell each. (Phase 1 spike;
//! the sidebar, tabs and splits come next.)

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSBackingStoreType, NSEventModifierFlags, NSMenu, NSMenuItem,
    NSRequestUserAttentionType, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSPoint, NSRect, NSSize, NSString};

use super::view::{TermView, ViewEvent};

const APP_NAME: &str = "Odek";

thread_local! {
    static INSTANCE: OnceCell<Retained<TermApp>> = const { OnceCell::new() };
}

#[allow(dead_code)]
fn instance() -> Option<Retained<TermApp>> {
    INSTANCE.with(|i| i.get().cloned())
}

struct Pane {
    window: Retained<NSWindow>,
    view: Retained<TermView>,
}

pub struct Ivars {
    start_dir: PathBuf,
    panes: RefCell<Vec<Pane>>,
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
            let dir = self.ivars().start_dir.clone();
            self.open_window(&dir, None, true);
            app.activate();
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_last_window(&self, _app: &NSApplication) -> bool {
            true
        }
    }

    unsafe impl NSWindowDelegate for TermApp {
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
        #[unsafe(method(termNewWindow:))]
        fn menu_new_window(&self, _sender: Option<&AnyObject>) {
            let dir = self.key_view().and_then(|v| v.session_cwd()).unwrap_or_else(|| self.ivars().start_dir.clone());
            self.open_window(&dir, None, true);
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
                v.set_font_size(super::view::DEFAULT_FONT_SIZE);
            }
        }
    }
);

impl TermApp {
    pub fn new(start_dir: PathBuf, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { start_dir, panes: RefCell::new(Vec::new()) });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        INSTANCE.with(|i| {
            let _ = i.set(this.clone());
        });
        this
    }

    fn key_view(&self) -> Option<Retained<TermView>> {
        let app = NSApplication::sharedApplication(self.mtm());
        let key = app.keyWindow()?;
        self.ivars().panes.borrow().iter().find(|p| std::ptr::eq(&*p.window, &*key)).map(|p| p.view.clone())
    }

    fn zoom(&self, delta: f64) {
        if let Some(v) = self.key_view() {
            v.set_font_size(v.font_size() + delta);
        }
    }

    pub fn open_window(&self, dir: &Path, command: Option<&str>, show: bool) -> Retained<TermView> {
        let mtm = self.mtm();
        // 100×30 cells.
        let probe = TermView::new(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 100.0)), mtm);
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
        self.ivars().panes.borrow_mut().push(Pane { window, view: view.clone() });
        view
    }

    fn build_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let cmd = NSEventModifierFlags::Command;
        let opt = NSEventModifierFlags::Option;
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
        let bar = NSMenu::new(mtm);
        for m in [
            menu(
                APP_NAME,
                vec![
                    item(&format!("About {APP_NAME}"), Some(sel!(orderFrontStandardAboutPanel:)), "", cmd),
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
                    item("New Window", Some(sel!(termNewWindow:)), "n", cmd),
                    sep(),
                    item("Close Window", Some(sel!(performClose:)), "w", cmd),
                ],
            ),
            menu(
                "Edit",
                vec![
                    item("Copy", Some(sel!(copy:)), "c", cmd),
                    item("Paste", Some(sel!(paste:)), "v", cmd),
                    item("Select All", Some(sel!(selectAll:)), "a", cmd),
                    sep(),
                    item("Clear Scrollback", Some(sel!(clearScrollback:)), "k", cmd),
                    item("Emoji & Symbols", Some(sel!(orderFrontCharacterPalette:)), "", cmd),
                ],
            ),
            menu(
                "View",
                vec![
                    item("Bigger", Some(sel!(termZoomIn:)), "+", cmd),
                    item("Smaller", Some(sel!(termZoomOut:)), "-", cmd),
                    item("Actual Size", Some(sel!(termZoomReset:)), "0", cmd),
                ],
            ),
            menu("Window", vec![item("Minimize", Some(sel!(performMiniaturize:)), "m", cmd)]),
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

pub fn run(start_dir: PathBuf) {
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
    let delegate = TermApp::new(start_dir, mtm);
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
    use objc2::DefinedClass;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplication, NSBitmapImageFileType, NSView};
    use objc2_foundation::NSDictionary;

    use super::super::view::TermView;
    use super::TermApp;

    thread_local! {
        static STATE: RefCell<Option<(Retained<TermView>, Vec<String>, PathBuf)>> = const { RefCell::new(None) };
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
        let view = app.open_window(&dir, cmd.as_deref(), false);
        println!("SNAP pid={}", std::process::id());
        STATE.with(|s| *s.borrow_mut() = Some((view, steps, out)));
        next();
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
        let (verb, arg) = step.split_once(' ').unwrap_or((&step, ""));
        match verb {
            "wait" => return after(arg.parse().unwrap_or(1.0)),
            "keys" => {
                let text = arg.replace("\\r", "\r").replace("\\e", "\x1b").replace("\\t", "\t");
                view.write(text.as_bytes());
            }
            "insert" => view.commit_text(arg),
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
            "snap" => {
                snapshot(&view, &out.join(format!("{arg}.png")));
                std::fs::write(out.join(format!("{arg}.txt")), view.screen_text()).ok();
                println!(
                    "SNAP {arg}: footprint {:.1} MB, terminal {:.2} MB",
                    footprint_mb(),
                    view.mem_bytes() as f64 / 1048576.0
                );
            }
            "mem" => println!(
                "SNAP mem: footprint {:.1} MB, terminal {:.2} MB",
                footprint_mb(),
                view.mem_bytes() as f64 / 1048576.0
            ),
            "quit" => {
                view.shutdown();
                let mtm = objc2::MainThreadMarker::new().unwrap();
                NSApplication::sharedApplication(mtm).terminate(None);
                return;
            }
            other => eprintln!("SNAP unknown step {other}"),
        }
        after(0.05);
    }

    /// Same number Activity Monitor shows as "Memory".
    fn footprint_mb() -> f64 {
        let mut info: libc::rusage_info_v0 = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            libc::proc_pid_rusage(std::process::id() as i32, libc::RUSAGE_INFO_V0, (&raw mut info).cast())
        } == 0;
        if ok { info.ri_phys_footprint as f64 / 1048576.0 } else { 0.0 }
    }

    fn snapshot(view: &NSView, path: &std::path::Path) {
        let bounds = view.bounds();
        let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else { return };
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let props = NSDictionary::new();
        if let Some(data) = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) } {
            let _ = std::fs::write(path, data.to_vec());
        }
    }
}
