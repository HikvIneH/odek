//! Scripted UI smoke test, driven by environment variables. It renders the
//! window to PNGs off-screen, so it works without screen-recording access:
//!
//!   SELFTEST_DIR=/tmp/out SELFTEST_FILES=/abs/a.go,/abs/b.tsx SELFTEST_QUERY=menu \
//!     ./target/release/odek /abs/project

use std::fs;
use std::path::{Path, PathBuf};

use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameDarkAqua, NSApplication, NSBitmapImageFileType, NSEvent,
    NSEventModifierFlags, NSEventType, NSTextInputClient, NSView,
};
use objc2_foundation::{NSDictionary, NSString};

use super::App;

pub struct SelfTest {
    dir: PathBuf,
    files: Vec<PathBuf>,
    query: String,
    step: usize,
    pub launch_ms: u64,
}

impl SelfTest {
    pub fn from_env() -> Option<SelfTest> {
        let dir = PathBuf::from(std::env::var_os("SELFTEST_DIR")?);
        let files = std::env::var("SELFTEST_FILES")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        let query = std::env::var("SELFTEST_QUERY").unwrap_or_else(|_| "main".into());
        fs::create_dir_all(&dir).ok()?;
        Some(SelfTest {
            dir,
            files,
            query,
            step: 0,
            launch_ms: 0,
        })
    }
}

#[repr(C)]
#[derive(Default)]
struct RusageInfoV0 {
    uuid: [u8; 16],
    user_time: u64,
    system_time: u64,
    pkg_idle_wkups: u64,
    interrupt_wkups: u64,
    pageins: u64,
    wired_size: u64,
    resident_size: u64,
    phys_footprint: u64,
    proc_start_abstime: u64,
    proc_exit_abstime: u64,
}

unsafe extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV0) -> i32;
}

/// Same number Activity Monitor shows as "Memory".
pub fn footprint_mb() -> f64 {
    let mut info = RusageInfoV0::default();
    let ok = unsafe { proc_pid_rusage(std::process::id() as i32, 0, &mut info) } == 0;
    if ok {
        info.phys_footprint as f64 / 1048576.0
    } else {
        0.0
    }
}

fn snapshot(view: &NSView, path: &Path) {
    let bounds = view.bounds();
    let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else {
        return;
    };
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let props = NSDictionary::new();
    if let Some(data) = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
    {
        let _ = fs::write(path, data.to_vec());
        println!("SELFTEST snapshot {}", path.display());
    }
}

impl App {
    /// Called once the window is up; does nothing unless SELFTEST_DIR is set.
    pub(super) fn start_self_test(&self) {
        let mut t = self.ivars().selftest.borrow_mut();
        let Some(t) = t.as_mut() else { return };
        t.launch_ms = crate::STARTED.get().map_or(0, |s| s.elapsed().as_millis() as u64);
        self.schedule_self_test(0.5);
    }

    /// Type keys into the editor through real NSEvents (⎋ = Esc, ⏎ = Return),
    /// so the whole keyDown → Vim path runs. Turns Vim mode on first.
    fn run_vim_keys(&self, keys: &str) {
        let ui = self.ui();
        if !self.ivars().vim_on.get() {
            self.vim_toggle(None);
        }
        ui.window.as_ref().unwrap().makeFirstResponder(Some(&ui.text));
        for ch in keys.chars() {
            let (text, code): (String, u16) = match ch {
                '⎋' => ("\u{1b}".into(), 53),
                '⏎' => ("\r".into(), 36),
                c => (c.to_string(), 0),
            };
            let s = NSString::from_str(&text);
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown,
                objc2_foundation::NSPoint::new(0.0, 0.0),
                NSEventModifierFlags::empty(),
                0.0,
                ui.window.as_ref().unwrap().windowNumber(),
                None,
                &s,
                &s,
                false,
                code,
            );
            if let Some(event) = event {
                ui.text.keyDown(&event);
            }
        }
        let text = ui.text.string().to_string();
        let first: Vec<&str> = text.lines().take(4).collect();
        println!(
            "SELFTEST vim keys={keys:?} status={:?} cursor={} lines={first:?}",
            self.ivars().vim.borrow().status(),
            selected_range_of(&ui.text)
        );
    }

    pub(super) fn schedule_self_test(&self, delay: f64) {
        unsafe {
            let _: () = msg_send![self, performSelector: sel!(selfTestStep:), withObject: None::<&AnyObject>, afterDelay: delay];
        }
    }

    fn snap(&self, name: &str, panel: bool) {
        // Snapshot bitmaps are large; skip them when measuring memory.
        if std::env::var_os("SELFTEST_NOSNAP").is_some() {
            return;
        }
        let ui = self.ui();
        let dir = self.ivars().selftest.borrow().as_ref().map(|t| t.dir.clone());
        let Some(dir) = dir else { return };
        let window: &objc2_app_kit::NSWindow = if panel { &ui.panel } else { ui.window.as_ref().unwrap() };
        // The content view's superview is the frame view, which includes the title bar.
        if let Some(content) = window.contentView() {
            let view = unsafe { content.superview() }.unwrap_or(content);
            snapshot(&view, &dir.join(format!("{name}.png")));
        }
    }

    pub(super) fn run_self_test_step(&self) {
        let (step, files, query) = {
            let mut t = self.ivars().selftest.borrow_mut();
            let Some(t) = t.as_mut() else { return };
            t.step += 1;
            (t.step, t.files.clone(), t.query.clone())
        };
        let ui = self.ui();
        let n = files.len();
        println!("SELFTEST step {step} footprint {:.1} MB", footprint_mb());
        // SELFTEST_IDLE=N: after opening the files, sit idle N seconds
        // (sampling once a second above), then quit. Measures steady state.
        let idle: usize = std::env::var("SELFTEST_IDLE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if idle > 0 && step >= 2 + 2 * n {
            if step >= 2 + 2 * n + idle {
                println!(
                    "SELFTEST git title={:?} tip={:?}",
                    ui.git_button.title().to_string(),
                    ui.git_button.toolTip().map(|t| t.to_string())
                );
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            } else {
                self.schedule_self_test(1.0);
            }
            return;
        }
        match step {
            1 => {
                if let Some(t) = self.ivars().selftest.borrow().as_ref() {
                    println!("SELFTEST window shown {} ms after main()", t.launch_ms);
                }
                if std::env::var_os("SELFTEST_LIGHT").is_some() {
                    let app = NSApplication::sharedApplication(self.mtm());
                    app.setAppearance(
                        NSAppearance::appearanceNamed(unsafe { objc2_app_kit::NSAppearanceNameAqua })
                            .as_deref(),
                    );
                }
                self.snap("00-start", false)
            }
            s if s >= 2 && s < 2 + 2 * n => {
                let k = (s - 2) / 2;
                if (s - 2) % 2 == 0 {
                    self.open_file(&files[k], std::env::var_os("SELFTEST_PIN").is_none());
                    self.reveal(&files[k]);
                } else {
                    let name = files[k].file_name().unwrap().to_string_lossy().into_owned();
                    let tabs = self.ivars().tabs.borrow();
                    let t = tabs.current.map(|i| &tabs.list[i]);
                    println!(
                        "SELFTEST file {} chars={} spans={} lang={:?} tabs={}",
                        name,
                        t.map_or(0, |t| t.storage.length()),
                        t.map_or(0, |t| t.spans.len()),
                        t.and_then(|t| t.lang),
                        tabs.list.len()
                    );
                    drop(tabs);
                    let clip = ui.scroll.contentView().bounds();
                    let tf = ui.text.frame();
                    let cs = ui.container.size();
                    println!(
                        "SELFTEST geom clip=({:.0},{:.0} {:.0}x{:.0}) text=({:.0},{:.0} {:.0}x{:.0}) container={:.0}x{:.0} content={:.0} ruler={:.0}",
                        clip.origin.x,
                        clip.origin.y,
                        clip.size.width,
                        clip.size.height,
                        tf.origin.x,
                        tf.origin.y,
                        tf.size.width,
                        tf.size.height,
                        cs.width,
                        cs.height.min(1e9),
                        ui.scroll.contentSize().width,
                        ui.ruler.ruleThickness()
                    );
                    self.snap(&format!("{:02}-{name}", k + 1), false);
                }
            }
            s if s == 2 + 2 * n && std::env::var_os("SELFTEST_VIM").is_some() && n > 0 => {
                self.run_vim_keys(&std::env::var("SELFTEST_VIM").unwrap_or_default());
                self.snap("80-vim", false);
                // Skip quick open; go straight to quitting.
                if let Some(t) = self.ivars().selftest.borrow_mut().as_mut() {
                    t.step = 4 + 2 * n;
                }
            }
            s if s == 2 + 2 * n => {
                self.show_quick_open();
                ui.field.setStringValue(&NSString::from_str(&query));
                self.quick_search();
            }
            s if s == 3 + 2 * n => {
                let q = self.ivars().quick.borrow();
                let top: Vec<&str> = q.hits.iter().take(5).map(|&i| q.files[i].as_str()).collect();
                println!("SELFTEST quick files={} top={:?}", q.files.len(), top);
                println!(
                    "SELFTEST git title={:?} hidden={} tip={:?}",
                    ui.git_button.title().to_string(),
                    ui.git_button.isHidden(),
                    ui.git_button.toolTip().map(|t| t.to_string())
                );
                drop(q);
                self.snap("90-quick-open", true);
                ui.panel.orderOut(None);
                let app = NSApplication::sharedApplication(self.mtm());
                app.setAppearance(
                    NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }).as_deref(),
                );
            }
            s if s == 4 + 2 * n => {
                self.snap("91-dark", false);
                // Edit in memory, check the dirty marker, then undo.
                if self.ivars().tabs.borrow().current.is_some() {
                    ui.text.setSelectedRange(objc2_foundation::NSRange::new(0, 0));
                    unsafe {
                        ui.text.insertText_replacementRange(
                            &NSString::from_str("x"),
                            objc2_foundation::NSRange::new(0, 0),
                        )
                    };
                    let dirty = self.ivars().tabs.borrow().list.iter().any(|t| t.dirty);
                    println!("SELFTEST dirty-after-edit={dirty}");
                    self.snap("92-dirty", false);
                    for t in self.ivars().tabs.borrow_mut().list.iter_mut() {
                        t.dirty = false;
                    }
                }
                println!("SELFTEST pid={} done; waiting 4s", std::process::id());
            }
            s if s == 5 + 2 * n => {
                NSApplication::sharedApplication(self.mtm()).terminate(None);
                return;
            }
            _ => {}
        }
        let delay = if step == 4 + 2 * n { 4.0 } else { 0.6 };
        self.schedule_self_test(delay);
    }
}

fn selected_range_of(tv: &objc2_app_kit::NSTextView) -> usize {
    super::selected_range(tv).location
}
