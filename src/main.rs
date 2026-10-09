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
    // `odek --term [dir]`, or the binary installed as `odek-term`: terminal
    // windows (in progress).
    let exe = std::env::args().next().unwrap_or_default();
    let as_term = exe.rsplit('/').next() == Some("odek-term");
    let mut args = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("-psn"))
        .peekable();
    if as_term || args.peek().map(String::as_str) == Some("--term") {
        if !as_term {
            args.next();
        }
        return term::app::run(args.next().map(std::path::PathBuf::from));
    }
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = app::App::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
