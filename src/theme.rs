//! Syntax colours: GitHub's light and dark palettes, as dynamic NSColors so
//! they follow the system appearance live.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor};
use objc2_foundation::NSArray;

use crate::highlight::Color;

/// (light, dark) sRGB hex per highlight colour, in `Color` order.
const PALETTE: [(Color, u32, u32); 10] = [
    (Color::Comment, 0x6E7781, 0x8B949E),
    (Color::Keyword, 0xCF222E, 0xFF7B72),
    (Color::String, 0x0A3069, 0xA5D6FF),
    (Color::Number, 0x0550AE, 0x79C0FF),
    (Color::Function, 0x8250DF, 0xD2A8FF),
    (Color::Type, 0x953800, 0xFFA657),
    (Color::Property, 0x0550AE, 0x79C0FF),
    (Color::Escape, 0x116329, 0x7EE787),
    (Color::Heading, 0x0550AE, 0x79C0FF),
    (Color::Link, 0x0A3069, 0xA5D6FF),
];

fn rgb(hex: u32) -> Retained<NSColor> {
    let c = |shift: u32| ((hex >> shift) & 0xFF) as f64 / 255.0;
    NSColor::colorWithSRGBRed_green_blue_alpha(c(16), c(8), c(0), 1.0)
}

fn dynamic(light: u32, dark: u32) -> Retained<NSColor> {
    let (light, dark) = (rgb(light), rgb(dark));
    let provider = RcBlock::new(move |appearance: NonNull<NSAppearance>| -> NonNull<NSColor> {
        let appearance = unsafe { appearance.as_ref() };
        let names = unsafe { NSArray::from_slice(&[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]) };
        let is_dark = appearance
            .bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|n| n.isEqualToString(unsafe { NSAppearanceNameDarkAqua }));
        // The block owns both colours, so a +0 pointer stays valid.
        NonNull::from(&**if is_dark { &dark } else { &light })
    });
    unsafe { NSColor::colorWithName_dynamicProvider(None, &provider) }
}

/// One colour per `Color` variant, indexable by `color as usize`.
pub fn palette() -> Vec<Retained<NSColor>> {
    PALETTE
        .iter()
        .enumerate()
        .map(|(i, &(c, light, dark))| {
            debug_assert_eq!(c as usize, i);
            dynamic(light, dark)
        })
        .collect()
}
