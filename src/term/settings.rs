//! The Settings window (⌘,) and the saved preferences it edits. Values live
//! in NSUserDefaults; changes apply to every open terminal at once, and new
//! terminals read them when they are created.

use std::cell::RefCell;
use std::collections::BTreeSet;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSControl, NSControlStateValueOn, NSFont, NSFontManager,
    NSFontTraitMask, NSPopUpButton, NSSlider, NSTextAlignment, NSTextField, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSUserDefaults};

use super::target::Target;
use super::view::{DEFAULT_FONT_SIZE, TermView};
use super::vt::{DEFAULT_SCROLLBACK, DEFAULT_SCROLLBACK_BYTES, Term};

const FONT: &str = "terminalFont";
const SIZE: &str = "terminalFontSize";
const THEME: &str = "terminalTheme";
const SCROLLBACK: &str = "terminalScrollback";
const OPTION_META: &str = "optionAsMeta";
const OPACITY: &str = "terminalOpacity";
const BLUR: &str = "terminalBlur";

const AUTOMATIC: &str = "Automatic (Nerd Font if installed)";
const SIZES: std::ops::RangeInclusive<isize> = 9..=24;
const SCROLLBACKS: [usize; 4] = [1_000, 5_000, 10_000, 50_000];
/// Opacity in percent; below the minimum text gets hard to read.
const OPACITY_RANGE: std::ops::RangeInclusive<isize> = 50..=100;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ThemePref {
    System,
    Light,
    Dark,
}

impl ThemePref {
    const ALL: [(ThemePref, &'static str); 3] = [
        (ThemePref::System, "System"),
        (ThemePref::Light, "Light"),
        (ThemePref::Dark, "Dark"),
    ];

    fn parse(s: &str) -> ThemePref {
        Self::ALL
            .iter()
            .find(|(_, n)| *n == s)
            .map_or(ThemePref::System, |(t, _)| *t)
    }

    fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(t, _)| *t == self)
            .map_or("System", |(_, n)| n)
    }
}

fn defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}

fn key(name: &str) -> Retained<NSString> {
    NSString::from_str(name)
}

fn string(name: &str) -> Option<String> {
    defaults().stringForKey(&key(name)).map(|s| s.to_string())
}

fn set_string(name: &str, value: Option<&str>) {
    match value {
        Some(v) if !v.is_empty() => unsafe { defaults().setObject_forKey(Some(&key(v)), &key(name)) },
        _ => defaults().removeObjectForKey(&key(name)),
    }
}

pub fn font_size() -> f64 {
    let n = defaults().integerForKey(&key(SIZE));
    if SIZES.contains(&n) {
        n as f64
    } else {
        DEFAULT_FONT_SIZE
    }
}

pub fn theme() -> ThemePref {
    ThemePref::parse(&string(THEME).unwrap_or_default())
}

pub fn scrollback() -> usize {
    let n = defaults().integerForKey(&key(SCROLLBACK));
    SCROLLBACKS
        .into_iter()
        .find(|&l| l as isize == n)
        .unwrap_or(DEFAULT_SCROLLBACK)
}

/// Terminal background opacity, 0.5–1.0 (stored as a percentage).
pub fn opacity() -> f64 {
    let n = defaults().integerForKey(&key(OPACITY));
    if OPACITY_RANGE.contains(&n) {
        n as f64 / 100.0
    } else {
        1.0
    }
}

/// Blur what's behind a translucent window (on unless turned off).
pub fn blur() -> bool {
    defaults().objectForKey(&key(BLUR)).is_none() || defaults().boolForKey(&key(BLUR))
}

pub fn option_as_meta() -> bool {
    defaults().objectForKey(&key(OPTION_META)).is_none() || defaults().boolForKey(&key(OPTION_META))
}

/// About 800 bytes a line, as for the default cap.
fn scrollback_bytes(lines: usize) -> usize {
    lines * DEFAULT_SCROLLBACK_BYTES / DEFAULT_SCROLLBACK
}

pub fn apply_scrollback(t: &mut Term) {
    let lines = scrollback();
    t.history.set_limits(lines, scrollback_bytes(lines));
}

fn apply_all() {
    for v in TermView::all() {
        v.apply_settings();
    }
    super::app::appearance_changed();
}

/// Families of the installed fixed-pitch fonts, sorted.
fn fixed_pitch_families() -> Vec<String> {
    let fm = NSFontManager::sharedFontManager(MainThreadMarker::new().expect("main thread"));
    let names = fm.availableFontNamesWithTraits(NSFontTraitMask::FixedPitchFontMask);
    let families: BTreeSet<String> = names
        .iter()
        .filter_map(|n| n.firstObject())
        .filter_map(|n| NSFont::fontWithName_size(&n, 12.0))
        .filter_map(|f| f.familyName())
        .map(|f| f.to_string())
        .filter(|f| !f.starts_with('.'))
        .collect();
    families.into_iter().collect()
}

struct Panel {
    window: Retained<NSWindow>,
    /// Controls hold their targets weakly.
    _targets: Vec<Retained<Target>>,
}

thread_local! {
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
}

pub fn show(mtm: MainThreadMarker) {
    let window = PANEL.with(|p| p.borrow_mut().get_or_insert_with(|| build(mtm)).window.clone());
    NSApplication::sharedApplication(mtm).activate();
    window.makeKeyAndOrderFront(None);
}

const WIDTH: f64 = 440.0;
const ROW: f64 = 34.0;
const LABEL_W: f64 = 120.0;
const MARGIN: f64 = 20.0;

fn build(mtm: MainThreadMarker) -> Panel {
    let height = 7.0 * ROW + 2.0 * MARGIN;
    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, height));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&key("Settings"));
    window.center();
    let content = NSView::initWithFrame(NSView::alloc(mtm), frame);
    window.setContentView(Some(&content));

    let mut targets = Vec::new();
    let mut row = 0.0;
    let mut place = |label: &str, control: &NSControl, width: f64| {
        let y = height - MARGIN - (row + 1.0) * ROW + 4.0;
        let l = NSTextField::labelWithString(&key(label), mtm);
        l.setFrame(NSRect::new(
            NSPoint::new(MARGIN, y + 3.0),
            NSSize::new(LABEL_W, 20.0),
        ));
        l.setAlignment(NSTextAlignment::Right);
        content.addSubview(&l);
        let x = MARGIN + LABEL_W + 10.0;
        control.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, 26.0)));
        content.addSubview(control);
        row += 1.0;
    };
    let wide = WIDTH - 2.0 * MARGIN - LABEL_W - 10.0;
    let popup = |items: &[String], selected: &str| {
        let p = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        for i in items {
            p.addItemWithTitle(&key(i));
        }
        p.selectItemWithTitle(&key(selected));
        p
    };
    let mut wire = |control: &NSControl, f: Box<dyn Fn()>| {
        let t = Target::new(mtm, move |_| f());
        unsafe {
            control.setTarget(Some(&t));
            control.setAction(Some(Target::action()));
        }
        targets.push(t);
    };

    let mut families = fixed_pitch_families();
    let saved = string(FONT).unwrap_or_default();
    if !saved.is_empty() && !families.contains(&saved) {
        families.push(saved.clone());
    }
    let mut items = vec![AUTOMATIC.to_string()];
    items.extend(families);
    let font = popup(&items, if saved.is_empty() { AUTOMATIC } else { &saved });
    let p = font.clone();
    wire(
        &font,
        Box::new(move || {
            let title = p.titleOfSelectedItem().map(|s| s.to_string());
            set_string(FONT, title.as_deref().filter(|t| *t != AUTOMATIC));
            apply_all();
        }),
    );
    place("Font", &font, wide);

    let sizes: Vec<String> = SIZES.map(|n| n.to_string()).collect();
    let size = popup(&sizes, &(font_size() as isize).to_string());
    let p = size.clone();
    wire(
        &size,
        Box::new(move || {
            let n = p
                .titleOfSelectedItem()
                .and_then(|s| s.to_string().parse::<isize>().ok());
            defaults().setInteger_forKey(n.unwrap_or(DEFAULT_FONT_SIZE as isize), &key(SIZE));
            apply_all();
        }),
    );
    place("Font size", &size, 80.0);

    let themes: Vec<String> = ThemePref::ALL.iter().map(|(_, n)| n.to_string()).collect();
    let theme_popup = popup(&themes, theme().name());
    let p = theme_popup.clone();
    wire(
        &theme_popup,
        Box::new(move || {
            set_string(THEME, p.titleOfSelectedItem().map(|s| s.to_string()).as_deref());
            apply_all();
        }),
    );
    place("Theme", &theme_popup, 120.0);

    let lines: Vec<String> = SCROLLBACKS.iter().map(|&n| lines_title(n)).collect();
    let scroll = popup(&lines, &lines_title(scrollback()));
    let p = scroll.clone();
    wire(
        &scroll,
        Box::new(move || {
            let i = p.indexOfSelectedItem().max(0) as usize;
            let n = SCROLLBACKS.get(i).copied().unwrap_or(DEFAULT_SCROLLBACK);
            defaults().setInteger_forKey(n as isize, &key(SCROLLBACK));
            apply_all();
        }),
    );
    place("Scrollback", &scroll, 150.0);

    // Opacity: a slider with the percentage beside it, applied while dragging.
    let percent = NSTextField::labelWithString(&key(&opacity_title(opacity())), mtm);
    let shown = percent.clone();
    let t = Target::new(mtm, move |sender| {
        if let Some(s) = sender.and_then(|s| s.downcast_ref::<NSSlider>()) {
            let n = s.doubleValue().round() as isize;
            defaults().setInteger_forKey(n, &key(OPACITY));
            shown.setStringValue(&key(&opacity_title(n as f64 / 100.0)));
            apply_all();
        }
    });
    let slider = unsafe {
        NSSlider::sliderWithValue_minValue_maxValue_target_action(
            opacity() * 100.0,
            *OPACITY_RANGE.start() as f64,
            *OPACITY_RANGE.end() as f64,
            Some(&t as &AnyObject),
            Some(Target::action()),
            mtm,
        )
    };
    targets.push(t);
    slider.setContinuous(true);
    place("Opacity", &slider, 180.0);
    // Same row as the slider: the fifth one.
    let row_y = height - MARGIN - 5.0 * ROW + 4.0;
    percent.setFrame(NSRect::new(
        NSPoint::new(MARGIN + LABEL_W + 10.0 + 190.0, row_y + 3.0),
        NSSize::new(60.0, 20.0),
    ));
    content.addSubview(&percent);

    let t = Target::new(mtm, |sender| {
        if let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) {
            defaults().setBool_forKey(b.state() == NSControlStateValueOn, &key(BLUR));
            apply_all();
        }
    });
    let blur_box = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &key("Blur what's behind the window"),
            Some(&t as &AnyObject),
            Some(Target::action()),
            mtm,
        )
    };
    targets.push(t);
    blur_box.setState(if blur() { NSControlStateValueOn } else { 0 });
    place("", &blur_box, wide);

    let t = Target::new(mtm, |sender| {
        if let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) {
            defaults().setBool_forKey(b.state() == NSControlStateValueOn, &key(OPTION_META));
        }
    });
    let meta = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &key("Option sends Meta (Esc+key)"),
            Some(&t as &AnyObject),
            Some(Target::action()),
            mtm,
        )
    };
    targets.push(t);
    meta.setState(if option_as_meta() {
        NSControlStateValueOn
    } else {
        0
    });
    place("Keyboard", &meta, wide);

    Panel {
        window,
        _targets: targets,
    }
}

fn opacity_title(o: f64) -> String {
    format!("{}%", (o * 100.0).round() as isize)
}

fn lines_title(n: usize) -> String {
    let s = n.to_string();
    let (head, tail) = s.split_at(s.len() - 3);
    format!("{head},{tail} lines")
}

/// Selftest: the settings window's content, to snapshot it.
#[cfg(feature = "selftest")]
pub fn content_view() -> Option<Retained<NSView>> {
    PANEL.with(|p| {
        p.borrow().as_ref().and_then(|p| {
            p.window.displayIfNeeded();
            p.window.contentView().and_then(|c| unsafe { c.superview() })
        })
    })
}

/// Selftest: set (or with no value, clear) a preference and apply it.
#[cfg(feature = "selftest")]
pub fn set_raw(name: &str, value: &str) {
    let value = (!value.is_empty()).then_some(value);
    match (name, value.and_then(|v| v.parse::<isize>().ok())) {
        (SIZE | SCROLLBACK | OPACITY, Some(n)) => defaults().setInteger_forKey(n, &key(name)),
        (OPTION_META | BLUR, _) => match value {
            Some(v) => defaults().setBool_forKey(v == "1", &key(name)),
            None => defaults().removeObjectForKey(&key(name)),
        },
        _ => set_string(name, value),
    }
    apply_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollback_bytes_scale() {
        assert_eq!(scrollback_bytes(10_000), DEFAULT_SCROLLBACK_BYTES);
        assert_eq!(scrollback_bytes(50_000), 5 * DEFAULT_SCROLLBACK_BYTES);
        assert!(scrollback_bytes(1_000) < scrollback_bytes(5_000));
    }

    #[test]
    fn titles_and_theme_names() {
        assert_eq!(lines_title(1_000), "1,000 lines");
        assert_eq!(lines_title(50_000), "50,000 lines");
        assert_eq!(ThemePref::parse("Dark"), ThemePref::Dark);
        assert_eq!(ThemePref::parse("junk"), ThemePref::System);
        assert_eq!(ThemePref::Light.name(), "Light");
        assert_eq!(opacity_title(0.85), "85%");
    }
}
