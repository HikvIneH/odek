//! Update check: at most once a day (and from the app menu) ask GitHub for the
//! latest release. A newer one retitles the app menu item and, once per
//! version, says so in an alert. The request runs the system `curl` off the
//! main thread, the way the git status bar runs `git`.
//!
//! Only bundled builds check on their own, so `cargo run` and the off-screen
//! snapshot runs never touch the network.

use std::cell::RefCell;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSMenuItem, NSWorkspace};
use objc2_foundation::{NSBundle, NSString, NSURL, NSUserDefaults};

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const API: &str = "https://api.github.com/repos/HikvIneH/odek/releases/latest";
const RELEASE_PAGE: &str = "https://github.com/HikvIneH/odek/releases/latest";
const EVERY_SECS: f64 = 24.0 * 60.0 * 60.0;
const AUTO_KEY: &str = "checkForUpdates";
const CHECKED_KEY: &str = "updateCheckedAt";
const ANNOUNCED_KEY: &str = "updateAnnounced";
pub const CHECK_TITLE: &str = "Check for Updates…";

thread_local! {
    /// The app menu's "Check for Updates…" item, retitled when one is out.
    static ITEM: RefCell<Option<Retained<NSMenuItem>>> = const { RefCell::new(None) };
}

// ---- pure decisions ----

/// The version in a GitHub "latest release" response: its `tag_name` without
/// the leading `v`.
pub fn tag_version(json: &str) -> Option<String> {
    let rest = &json[json.find("\"tag_name\"")? + "\"tag_name\"".len()..];
    let rest = rest
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let tag = &rest[..rest.find('"')?];
    let v = tag.strip_prefix('v').unwrap_or(tag);
    (!v.is_empty()).then(|| v.to_string())
}

/// Whether `latest` is a later version than `current` (dotted numbers; a
/// pre-release suffix such as `-beta` is ignored).
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(latest), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// Whether an automatic check is due.
pub fn due(now: f64, last_checked: f64) -> bool {
    now - last_checked >= EVERY_SECS || now < last_checked
}

// ---- AppKit ----

fn defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}

/// Automatic checks are on unless turned off from the app menu.
pub fn auto_enabled() -> bool {
    let key = NSString::from_str(AUTO_KEY);
    defaults().objectForKey(&key).is_none() || defaults().boolForKey(&key)
}

pub fn set_auto_enabled(on: bool) {
    defaults().setBool_forKey(on, &NSString::from_str(AUTO_KEY));
}

pub fn set_menu_item(item: Retained<NSMenuItem>) {
    ITEM.with(|i| *i.borrow_mut() = Some(item));
}

/// Called when the app becomes active: check if a day has passed.
pub fn on_activate() {
    if NSBundle::mainBundle().bundleIdentifier().is_none() || !auto_enabled() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let key = NSString::from_str(CHECKED_KEY);
    if !due(now, defaults().doubleForKey(&key)) {
        return;
    }
    defaults().setDouble_forKey(now, &key);
    check(false);
}

/// "Check for Updates…": always answers, even when there's nothing new.
pub fn check_now() {
    check(true);
}

fn check(manual: bool) {
    std::thread::spawn(move || {
        let latest = Command::new("/usr/bin/curl")
            .args([
                "-fsSL",
                "--max-time",
                "15",
                "-H",
                "Accept: application/vnd.github+json",
                "-A",
            ])
            .arg(format!("odek/{CURRENT}"))
            .arg(API)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| tag_version(&String::from_utf8_lossy(&o.stdout)));
        DispatchQueue::main().exec_async(move || finished(latest, manual));
    });
}

fn finished(latest: Option<String>, manual: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    match latest {
        Some(v) if is_newer(&v, CURRENT) => {
            ITEM.with(|i| {
                if let Some(item) = i.borrow().as_ref() {
                    item.setTitle(&NSString::from_str(&format!("Update to Odek {v}…")));
                }
            });
            let key = NSString::from_str(ANNOUNCED_KEY);
            let announced = defaults().stringForKey(&key).map(|s| s.to_string());
            if manual || announced.as_deref() != Some(v.as_str()) {
                unsafe { defaults().setObject_forKey(Some(&NSString::from_str(&v)), &key) };
                announce(mtm, &v);
            }
        }
        Some(_) if manual => {
            let info = format!("You have the latest version, {CURRENT}.");
            alert(mtm, "Odek is up to date", &info, &["OK"]);
        }
        None if manual => {
            let info = "GitHub didn't answer. Check your connection and try again.";
            alert(mtm, "Couldn't check for updates", info, &["OK"]);
        }
        _ => {}
    }
}

fn announce(mtm: MainThreadMarker, version: &str) {
    let info = format!(
        "You have {CURRENT}. If you installed Odek with Homebrew, update it with:\n\nbrew upgrade --cask odek"
    );
    if alert(
        mtm,
        &format!("Odek {version} is available"),
        &info,
        &["View Release", "Later"],
    ) {
        if let Some(url) = NSURL::URLWithString(&NSString::from_str(RELEASE_PAGE)) {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }
}

/// A modal alert; true when the first button was chosen.
fn alert(mtm: MainThreadMarker, message: &str, info: &str, buttons: &[&str]) -> bool {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(message));
    alert.setInformativeText(&NSString::from_str(info));
    for b in buttons {
        alert.addButtonWithTitle(&NSString::from_str(b));
    }
    alert.runModal() == NSAlertFirstButtonReturn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tag() {
        let json = r#"{"url":"x","tag_name": "v0.3.0","name":"odek 0.3.0"}"#;
        assert_eq!(tag_version(json).as_deref(), Some("0.3.0"));
        assert_eq!(tag_version(r#"{"tag_name":"1.0"}"#).as_deref(), Some("1.0"));
        assert_eq!(tag_version(r#"{"message":"Not Found"}"#), None);
        assert_eq!(tag_version(r#"{"tag_name":""}"#), None);
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.99.1"));
        assert!(is_newer("0.2.1", "0.2"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.2", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("0.3.0-beta", "0.3.0"));
    }

    #[test]
    fn checks_once_a_day() {
        assert!(due(100_000.0, 0.0));
        assert!(!due(100_000.0, 100_000.0 - 3600.0));
        assert!(due(100_000.0, 100_000.0 - EVERY_SECS));
        // A clock set back shouldn't silence checks for days.
        assert!(due(100.0, 100_000.0));
    }
}
