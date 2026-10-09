//! Drag and drop onto a terminal, as Terminal.app and Warp do it: files
//! become their shell-escaped paths; raw image data (from a browser or a
//! screenshot thumbnail) is saved as a PNG in the temp dir and becomes that
//! path, so tools like Claude Code can pick the image up.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::rc::Retained;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardType, NSPasteboardTypeFileURL,
    NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSArray, NSDictionary, NSURL};

/// The pasteboard types a terminal view accepts.
pub fn types() -> Retained<NSArray<NSPasteboardType>> {
    unsafe {
        NSArray::from_slice(&[
            NSPasteboardTypeFileURL,
            NSPasteboardTypePNG,
            NSPasteboardTypeTIFF,
            NSPasteboardTypeString,
        ])
    }
}

/// What dropping `pb` should type: escaped paths followed by a space, or the
/// dropped text as is.
pub fn text(pb: &NSPasteboard) -> Option<String> {
    let mut paths = Vec::new();
    for item in pb.pasteboardItems().unwrap_or_default() {
        let url = item
            .stringForType(unsafe { NSPasteboardTypeFileURL })
            .and_then(|s| NSURL::URLWithString(&s))
            .and_then(|u| u.path());
        if let Some(p) = url {
            paths.push(p.to_string());
        } else if let Some(p) = save_image(&item) {
            paths.push(p.to_string_lossy().into_owned());
        }
    }
    if !paths.is_empty() {
        let quoted: Vec<String> = paths.iter().map(|p| quote(p)).collect();
        return Some(format!("{} ", quoted.join(" ")));
    }
    pb.stringForType(unsafe { NSPasteboardTypeString })
        .map(|s| s.to_string())
}

/// Writes an item's PNG or TIFF data to a fresh PNG file.
fn save_image(item: &objc2_app_kit::NSPasteboardItem) -> Option<PathBuf> {
    let png = match item.dataForType(unsafe { NSPasteboardTypePNG }) {
        Some(d) => d,
        None => {
            let tiff = item.dataForType(unsafe { NSPasteboardTypeTIFF })?;
            let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
            let props = NSDictionary::new();
            unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?
        }
    };
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_millis();
    let path = std::env::temp_dir().join(format!("odek-drop-{ms}.png"));
    std::fs::write(&path, png.to_vec()).ok()?;
    Some(path)
}

/// Backslash-escapes what the shell would treat specially, like Terminal.app.
pub fn quote(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if !(c.is_alphanumeric() || "-_./,:@+=%".contains(c)) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn quotes_shell_specials() {
        assert_eq!(quote("/tmp/a.png"), "/tmp/a.png");
        assert_eq!(
            quote("/Users/me/Screen Shot (1).png"),
            "/Users/me/Screen\\ Shot\\ \\(1\\).png"
        );
        assert_eq!(quote("/x/it's $HOME&;"), "/x/it\\'s\\ \\$HOME\\&\\;");
        assert_eq!(quote("/x/foto café.jpg"), "/x/foto\\ café.jpg");
    }
}
