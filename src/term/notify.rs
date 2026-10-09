//! Desktop notifications for background tabs: when a pane rings the bell or
//! sends OSC 9 / 777 and the user can't see it, post a banner. Clicking it
//! brings that tab and pane forward.
//!
//! UserNotifications only works from an app bundle (a bare `cargo run` binary
//! raises), so everything is skipped without a bundle identifier and the
//! caller falls back to the Dock bounce. All state lives on the main thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, define_class};
use objc2_foundation::{NSBundle, NSDictionary, NSError, NSString, NSUserDefaults};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

use super::workspace::Id;

/// At most one notification per tab in this window.
const MIN_GAP: Duration = Duration::from_secs(5);
const DEFAULTS_KEY: &str = "notifyAttention";
const PANE_KEY: &str = "pane";

// ---- pure decisions ----

/// Only bother the user about what they can't already see.
pub fn should_notify(app_active: bool, window_key: bool, tab_active: bool) -> bool {
    !app_active || !window_key || !tab_active
}

/// Title: the tab's name, else its folder, else the app.
pub fn title_for(tab_title: &str, dir: &str) -> String {
    let t = tab_title.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    let d = dir.trim();
    if d.is_empty() {
        "Odek".to_string()
    } else {
        d.to_string()
    }
}

/// Body: the program's own message (OSC 9/777), else a generic line for a bell.
pub fn body_for(message: Option<&str>) -> String {
    match message.map(str::trim) {
        Some(m) if !m.is_empty() => m.chars().take(200).collect(),
        _ => "Needs your attention".to_string(),
    }
}

/// Per-tab rate limit.
#[derive(Default)]
pub struct Limiter {
    last: HashMap<Id, Instant>,
}

impl Limiter {
    pub fn allow(&mut self, tab: Id, now: Instant) -> bool {
        self.last
            .retain(|_, t| now.saturating_duration_since(*t) < MIN_GAP * 12);
        match self.last.get(&tab) {
            Some(t) if now.saturating_duration_since(*t) < MIN_GAP => false,
            _ => {
                self.last.insert(tab, now);
                true
            }
        }
    }
}

// ---- preference ----

pub fn enabled() -> bool {
    let d = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str(DEFAULTS_KEY);
    d.objectForKey(&key).is_none() || d.boolForKey(&key)
}

pub fn set_enabled(on: bool) {
    NSUserDefaults::standardUserDefaults().setBool_forKey(on, &NSString::from_str(DEFAULTS_KEY));
}

// ---- UserNotifications ----

define_class!(
    /// Shows banners while Odek is frontmost and routes clicks to the pane.
    #[unsafe(super(NSObject))]
    #[name = "OdekNotifyDelegate"]
    struct NotifyDelegate;

    unsafe impl NSObjectProtocol for NotifyDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotifyDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _n: &UNNotification,
            done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            done.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            done: &block2::DynBlock<dyn Fn()>,
        ) {
            let info = response.notification().request().content().userInfo();
            let pane = info
                .objectForKey(&NSString::from_str(PANE_KEY))
                .and_then(|o| o.downcast::<NSString>().ok())
                .and_then(|s| s.to_string().parse::<Id>().ok());
            // May arrive on a background queue.
            DispatchQueue::main().exec_async(move || super::app::reveal_pane(pane));
            done.call(());
        }
    }
);

#[derive(Clone, Copy, PartialEq)]
enum Auth {
    Unknown,
    Asking,
    Granted,
    Denied,
}

struct State {
    auth: Auth,
    limiter: Limiter,
    delegate: Option<Retained<NotifyDelegate>>,
    /// Posted while the permission prompt was open: (tab, pane, title, body).
    waiting: Vec<(Id, Id, String, String)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        auth: Auth::Unknown,
        limiter: Limiter::default(),
        delegate: None,
        waiting: Vec::new(),
    });
}

fn has_bundle() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

fn center() -> Option<Retained<UNUserNotificationCenter>> {
    has_bundle().then(UNUserNotificationCenter::currentNotificationCenter)
}

fn identifier(tab: Id) -> Retained<NSString> {
    NSString::from_str(&format!("odek-tab-{tab}"))
}

/// Install the delegate at launch so a click that starts the app is routed.
pub fn setup() {
    let Some(center) = center() else { return };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.delegate.is_none() {
            let d: Retained<NotifyDelegate> = unsafe { objc2::msg_send![NotifyDelegate::alloc(), init] };
            center.setDelegate(Some(ProtocolObject::from_ref(&*d)));
            s.delegate = Some(d);
        }
    });
}

/// Post (or replace) the notification for `tab`. Returns false when it can't
/// be used (disabled, no bundle, denied) so the caller keeps the Dock bounce.
pub fn post(tab: Id, pane: Id, title: &str, body: &str) -> bool {
    if !enabled() {
        return false;
    }
    if !has_bundle() {
        #[cfg(feature = "selftest")]
        eprintln!("[notify] skipped, no bundle id: tab={tab} pane={pane} title={title:?} body={body:?}");
        return false;
    }
    setup();
    let auth = STATE.with(|s| s.borrow().auth);
    match auth {
        Auth::Denied => false,
        Auth::Asking => {
            STATE.with(|s| {
                s.borrow_mut()
                    .waiting
                    .push((tab, pane, title.into(), body.into()))
            });
            true
        }
        Auth::Granted => {
            if STATE.with(|s| s.borrow_mut().limiter.allow(tab, Instant::now())) {
                deliver(tab, pane, title, body);
            }
            true
        }
        Auth::Unknown => {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.auth = Auth::Asking;
                s.waiting.push((tab, pane, title.into(), body.into()));
            });
            ask();
            true
        }
    }
}

fn ask() {
    let Some(center) = center() else { return };
    let block = RcBlock::new(|granted: Bool, _err: *mut NSError| {
        let granted = granted.as_bool();
        DispatchQueue::main().exec_async(move || authorised(granted));
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &block,
    );
}

fn authorised(granted: bool) {
    let waiting = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.auth = if granted { Auth::Granted } else { Auth::Denied };
        std::mem::take(&mut s.waiting)
    });
    if !granted {
        return;
    }
    for (tab, pane, title, body) in waiting {
        if STATE.with(|s| s.borrow_mut().limiter.allow(tab, Instant::now())) {
            deliver(tab, pane, &title, &body);
        }
    }
}

fn deliver(tab: Id, pane: Id, title: &str, body: &str) {
    let Some(center) = center() else { return };
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setThreadIdentifier(&identifier(tab));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    let key = NSString::from_str(PANE_KEY);
    let value: Retained<AnyObject> = NSString::from_str(&pane.to_string()).into();
    let info = NSDictionary::from_retained_objects(&[&*key], &[value]);
    // SAFETY: an untyped NSDictionary is the same object.
    unsafe { content.setUserInfo(info.cast_unchecked()) };
    // Same identifier per tab: the new banner replaces the old one.
    let request =
        UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier(tab), &content, None);
    center.addNotificationRequest_withCompletionHandler(&request, None);
}

/// The tab was visited: drop its banner from Notification Center.
pub fn clear(tab: Id) {
    let Some(center) = center() else { return };
    let ids = objc2_foundation::NSArray::from_retained_slice(&[identifier(tab)]);
    center.removeDeliveredNotificationsWithIdentifiers(&ids);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_when_unseen() {
        assert!(!should_notify(true, true, true));
        assert!(should_notify(false, true, true));
        assert!(should_notify(true, false, true));
        assert!(should_notify(true, true, false));
    }

    #[test]
    fn title_and_body() {
        assert_eq!(title_for("✳ fix login bug", "~/x"), "✳ fix login bug");
        assert_eq!(title_for("  ", "~/x"), "~/x");
        assert_eq!(title_for("", ""), "Odek");
        assert_eq!(body_for(None), "Needs your attention");
        assert_eq!(body_for(Some("  ")), "Needs your attention");
        assert_eq!(body_for(Some(" done ")), "done");
        assert_eq!(body_for(Some(&"x".repeat(500))).chars().count(), 200);
    }

    #[test]
    fn rate_limit_per_tab() {
        let mut l = Limiter::default();
        let t0 = Instant::now();
        assert!(l.allow(1, t0));
        assert!(!l.allow(1, t0 + Duration::from_secs(2)));
        assert!(l.allow(2, t0 + Duration::from_secs(2)));
        assert!(l.allow(1, t0 + Duration::from_secs(6)));
        assert!(!l.allow(1, t0 + Duration::from_secs(7)));
    }
}
