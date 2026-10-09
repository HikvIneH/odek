//! Drag and drop onto a terminal, as Terminal.app and Warp do it: files
//! become their shell-escaped paths; raw image data (from a browser) is saved
//! as a PNG in the temp dir and becomes that path, so tools like Claude Code
//! can pick the image up. Promised files (the screenshot thumbnail, Photos,
//! Mail attachments: the file doesn't exist until it's dropped) are written
//! to a fresh temp folder and become their paths once written.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{ClassType, Message};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSFilePromiseReceiver, NSPasteboard, NSPasteboardType,
    NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSOperationQueue, NSURL};

/// The pasteboard types a terminal view accepts.
pub fn types() -> Retained<NSArray<NSPasteboardType>> {
    let mut types: Vec<Retained<NSPasteboardType>> = unsafe {
        vec![
            NSPasteboardTypeFileURL.retain(),
            NSPasteboardTypePNG.retain(),
            NSPasteboardTypeTIFF.retain(),
            NSPasteboardTypeString.retain(),
        ]
    };
    types.extend(NSFilePromiseReceiver::readableDraggedTypes().iter());
    NSArray::from_retained_slice(&types)
}

/// Ask the sources of promised files in `pb` to write them, then call
/// `typed` on the main thread with each file's escaped path and a space.
/// False when `pb` holds no promises.
pub fn receive_promises(pb: &NSPasteboard, typed: impl Fn(String) + 'static) -> bool {
    let classes = NSArray::from_slice(&[NSFilePromiseReceiver::class()]);
    let Some(promises) = (unsafe { pb.readObjectsForClasses_options(&classes, None) }) else {
        return false;
    };
    let promises: Vec<_> = promises
        .iter()
        .filter_map(|o| o.downcast::<NSFilePromiseReceiver>().ok())
        .collect();
    if promises.is_empty() {
        return false;
    }
    let Some(ms) = SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis()) else {
        return false;
    };
    let dir = std::env::temp_dir().join(format!("odek-drop-{ms}"));
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let Some(dest) = NSURL::from_directory_path(&dir) else {
        return false;
    };
    let typed = std::rc::Rc::new(typed);
    let queue = NSOperationQueue::mainQueue();
    for p in promises {
        let typed = typed.clone();
        let reader = RcBlock::new(move |url: NonNull<NSURL>, err: *mut NSError| {
            if err.is_null()
                && let Some(path) = unsafe { url.as_ref() }.path()
            {
                typed(format!("{} ", quote(&path.to_string())));
            }
        });
        unsafe {
            p.receivePromisedFilesAtDestination_options_operationQueue_reader(
                &dest,
                &NSDictionary::new(),
                &queue,
                &reader,
            )
        };
    }
    true
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
