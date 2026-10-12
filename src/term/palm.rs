//! Ignore the trackpad while typing (Settings ▸ Trackpad, on by default): for a moment after
//! each key press, a palm brushing the trackpad can't click (moving focus to
//! another pane, selecting, placing the cursor) or scroll in odek's windows.
//! One local event monitor sees events before any view does and drops those;
//! the pointer itself still moves, which only the system can stop.

use std::cell::Cell;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};

use super::settings;

/// How long after a key press the trackpad is ignored.
const QUIET: Duration = Duration::from_millis(600);

thread_local! {
    static LAST_KEY: Cell<Option<Instant>> = const { Cell::new(None) };
    /// A click was dropped: drop its drags and release too.
    static SWALLOWING: Cell<bool> = const { Cell::new(false) };
    static MONITOR: Cell<Option<Retained<AnyObject>>> = const { Cell::new(None) };
}

/// Whether input arriving `now` is too soon after the last key press.
fn too_soon(last_key: Option<Instant>, now: Instant) -> bool {
    last_key.is_some_and(|k| now.saturating_duration_since(k) < QUIET)
}

/// Watch the app's events. Cheap: a time check per click or scroll.
pub fn install() {
    let mask = NSEventMask::KeyDown
        | NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::LeftMouseUp
        | NSEventMask::RightMouseDown
        | NSEventMask::RightMouseUp
        | NSEventMask::OtherMouseDown
        | NSEventMask::OtherMouseUp
        | NSEventMask::ScrollWheel;
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        let e = unsafe { event.as_ref() };
        if keep(e) {
            event.as_ptr()
        } else {
            std::ptr::null_mut()
        }
    });
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    MONITOR.with(|m| m.set(monitor));
}

/// False for an event to drop.
fn keep(e: &NSEvent) -> bool {
    keep_at(e, Instant::now(), settings::ignore_trackpad_while_typing())
}

fn keep_at(e: &NSEvent, now: Instant, enabled: bool) -> bool {
    let kind = e.r#type();
    if kind == NSEventType::KeyDown {
        // Typing, not shortcuts.
        if !e.modifierFlags().contains(NSEventModifierFlags::Command) {
            LAST_KEY.with(|k| k.set(Some(now)));
        }
        return true;
    }
    let ends_click = matches!(
        kind,
        NSEventType::LeftMouseUp | NSEventType::RightMouseUp | NSEventType::OtherMouseUp
    );
    if SWALLOWING.with(Cell::get) {
        if ends_click {
            SWALLOWING.with(|s| s.set(false));
        }
        return false;
    }
    if !enabled || !too_soon(LAST_KEY.with(Cell::get), now) {
        return true;
    }
    match kind {
        // Trackpads (and Magic Mice) scroll with precise deltas; a wheel
        // mouse doesn't, and keeps working.
        NSEventType::ScrollWheel => !e.hasPreciseScrollingDeltas(),
        NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown => {
            SWALLOWING.with(|s| s.set(true));
            false
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::{NSPoint, NSString};

    fn key(cmd: bool) -> Retained<NSEvent> {
        let flags = if cmd {
            NSEventModifierFlags::Command
        } else {
            NSEventModifierFlags::empty()
        };
        let s = NSString::from_str("a");
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown, NSPoint::new(0.0, 0.0), flags, 0.0, 0, None, &s, &s, false, 0,
        )
        .unwrap()
    }

    fn mouse(kind: NSEventType) -> Retained<NSEvent> {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind, NSPoint::new(10.0, 10.0), NSEventModifierFlags::empty(), 0.0, 0, None, 0, 1, 1.0,
        )
        .unwrap()
    }

    /// A tap during typing is dropped whole (down, drag, up); one after the
    /// quiet time, or with the setting off, gets through.
    #[test]
    fn drops_a_click_just_after_typing() {
        LAST_KEY.with(|k| k.set(None));
        SWALLOWING.with(|s| s.set(false));
        let t = Instant::now();
        let click = |at: Instant, enabled: bool| {
            [
                NSEventType::LeftMouseDown,
                NSEventType::LeftMouseDragged,
                NSEventType::LeftMouseUp,
            ]
            .map(|k| keep_at(&mouse(k), at, enabled))
        };
        assert_eq!(click(t, true), [true; 3], "nothing typed yet");
        assert!(keep_at(&key(false), t, true), "keys always go through");
        assert_eq!(click(t + Duration::from_millis(100), true), [false; 3]);
        assert_eq!(
            click(t + Duration::from_millis(100), false),
            [true; 3],
            "setting off"
        );
        assert_eq!(click(t + QUIET, true), [true; 3]);
        // A ⌘-shortcut isn't typing: a click right after it works.
        LAST_KEY.with(|k| k.set(None));
        assert!(keep_at(&key(true), t, true));
        assert_eq!(click(t, true), [true; 3]);
        // Right clicks too.
        assert!(keep_at(&key(false), t, true));
        assert!(!keep_at(&mouse(NSEventType::RightMouseDown), t, true));
        assert!(!keep_at(&mouse(NSEventType::RightMouseUp), t, true));
        assert!(keep_at(&mouse(NSEventType::RightMouseDown), t + QUIET, true));
    }

    #[test]
    fn only_just_after_a_key_press() {
        let t = Instant::now();
        assert!(!too_soon(None, t), "no key pressed yet");
        assert!(too_soon(Some(t), t));
        assert!(too_soon(Some(t), t + Duration::from_millis(599)));
        assert!(!too_soon(Some(t), t + QUIET));
        assert!(!too_soon(Some(t), t + Duration::from_secs(5)));
        // A key press stamped after `now` (clock order) counts as just now.
        assert!(too_soon(Some(t + Duration::from_millis(5)), t));
    }
}
