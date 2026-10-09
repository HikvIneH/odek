mod app;
mod codeview;
mod edit;
mod fuzzy;
mod git;
mod highlight;
mod ruler;
mod term;
mod theme;
mod tree;
mod vim;

use objc2::MainThreadMarker;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

/// Process start, for the self-test's launch-time report.
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    STARTED.get_or_init(std::time::Instant::now);
    // `odek [folder|file]` is the terminal; `odek --viewer [folder|file]`
    // the code viewer in a window of its own.
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("-psn"))
        .collect();
    if args.first().map(String::as_str) != Some("--viewer") {
        let open = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(std::path::PathBuf::from);
        return term::app::run(open);
    }
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = app::App::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
