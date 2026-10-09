//! Status-bar git branch: name, ↓behind ↑ahead, dirty dot, and a click menu
//! with Fetch / Pull. Git runs on a background thread; results hop back to
//! the main thread through the main dispatch queue.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use dispatch2::DispatchQueue;
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, sel};
use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSMenu, NSMenuItem,
};
use objc2_foundation::{NSAttributedString, NSPoint, NSString};

use super::{App, attrs};
use crate::git;

/// Re-fetch when the app comes to the front if the last fetch is older.
const FETCH_EVERY: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub struct GitState {
    /// Bumped when the project changes, so late results are dropped.
    generation: u64,
    repo: Option<PathBuf>,
    status: Option<git::Status>,
    last_fetch: Option<Instant>,
    fetch_error: Option<String>,
    busy: bool,
}

enum Job {
    Status,
    Fetch,
    Pull,
}

struct Outcome {
    generation: u64,
    repo: Option<PathBuf>,
    status: Option<git::Status>,
    fetched: bool,
    error: Option<String>,
    pulled: Option<Result<String, String>>,
    /// False for the early local-status update sent before a fetch/pull.
    done: bool,
}

fn ago(t: Instant) -> String {
    match t.elapsed().as_secs() {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s => format!("{} h ago", s / 3600),
    }
}

impl App {
    /// New project: forget the old repo, show local status, then fetch.
    pub(super) fn git_project_changed(&self) {
        {
            let mut g = self.ivars().git.borrow_mut();
            let generation = g.generation + 1;
            *g = GitState {
                generation,
                ..GitState::default()
            };
        }
        self.git_update_ui();
        self.git_run(Job::Fetch);
    }

    /// App came to the front: cheap local refresh, fetch if it's been a while.
    pub(super) fn git_on_activate(&self) {
        let stale = self
            .ivars()
            .git
            .borrow()
            .last_fetch
            .is_none_or(|t| t.elapsed() > FETCH_EVERY);
        self.git_run(if stale { Job::Fetch } else { Job::Status });
    }

    pub(super) fn git_after_save(&self) {
        self.git_run(Job::Status);
    }

    pub(super) fn git_fetch_now(&self) {
        self.git_run(Job::Fetch);
    }

    pub(super) fn git_pull_now(&self) {
        if self.ivars().tabs.borrow().list.iter().any(|t| t.dirty) {
            self.alert(
                "Save your changes first",
                "Pull would change files you have unsaved edits in.",
                &["OK"],
            );
            return;
        }
        self.git_run(Job::Pull);
    }

    fn git_run(&self, job: Job) {
        let dir = match self.ivars().tree.borrow().as_ref() {
            Some(t) => t.root().to_path_buf(),
            None => return,
        };
        let generation = {
            let mut g = self.ivars().git.borrow_mut();
            if g.busy {
                return;
            }
            g.busy = true;
            g.generation
        };
        self.git_update_ui();
        std::thread::spawn(move || {
            let send = |out: Outcome| {
                DispatchQueue::main().exec_async(move || {
                    if let Some(app) = super::instance() {
                        app.git_finished(out);
                    }
                });
            };
            let repo = git::repo_root(&dir);
            let base = || Outcome {
                generation,
                repo: repo.clone(),
                status: None,
                fetched: false,
                error: None,
                pulled: None,
                done: true,
            };
            let Some(root) = repo.clone() else {
                return send(base());
            };
            let local = git::status(&root);
            if matches!(job, Job::Status) {
                return send(Outcome {
                    status: local,
                    ..base()
                });
            }
            // Show the branch now; the network step can take a while.
            send(Outcome {
                status: local,
                done: false,
                ..base()
            });
            let mut out = base();
            match job {
                Job::Fetch => {
                    out.fetched = true;
                    out.error = git::fetch(&root).err();
                }
                Job::Pull => out.pulled = Some(git::pull(&root)),
                Job::Status => {}
            }
            out.status = git::status(&root);
            send(out);
        });
    }

    fn git_finished(&self, out: Outcome) {
        let pulled = {
            let mut g = self.ivars().git.borrow_mut();
            if out.generation != g.generation {
                return;
            }
            g.busy = !out.done;
            g.repo = out.repo;
            g.status = out.status;
            if out.fetched {
                g.last_fetch = Some(Instant::now());
                g.fetch_error = out.error;
            }
            out.pulled
        };
        self.git_update_ui();
        match pulled {
            Some(Ok(_)) => {
                self.ivars().git.borrow_mut().last_fetch = Some(Instant::now());
                self.refresh_from_disk(); // new/changed files from the pull
            }
            Some(Err(e)) => {
                self.alert("Pull didn't run", &e, &["OK"]);
            }
            None => {}
        }
    }

    fn git_update_ui(&self) {
        let ui = self.ui();
        let g = self.ivars().git.borrow();
        let Some(st) = g.status.as_ref() else {
            ui.git_button.setHidden(true);
            return;
        };
        ui.git_button.setHidden(false);
        let mut title = st.label();
        if g.busy {
            title.push_str("  …");
        }
        let color = if st.behind > 0 {
            NSColor::systemOrangeColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        let font = NSFont::systemFontOfSize(11.5);
        let a = attrs(&[
            (unsafe { NSFontAttributeName }, &*font),
            (unsafe { NSForegroundColorAttributeName }, &*color),
        ]);
        let text = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(&title),
                Some(&a),
            )
        };
        ui.git_button.setAttributedTitle(&text);
        ui.git_button.setContentTintColor(Some(&color));
        ui.git_button.sizeToFit();

        let mut tip = format!("Branch {}", st.branch);
        match &st.upstream {
            Some(u) if st.behind > 0 => {
                tip.push_str(&format!(" is {} commit(s) behind {u}: pull to update", st.behind))
            }
            Some(u) => tip.push_str(&format!(" tracks {u}")),
            None => tip.push_str(" has no upstream"),
        }
        if st.ahead > 0 {
            tip.push_str(&format!("; {} local commit(s) not pushed", st.ahead));
        }
        if st.dirty {
            tip.push_str("; uncommitted changes");
        }
        ui.git_button.setToolTip(Some(&NSString::from_str(&tip)));
    }

    /// Popup under the branch button.
    pub(super) fn git_show_menu(&self) {
        let ui = self.ui();
        let mtm = self.mtm();
        let (upstream, fetched, behind, busy) = {
            let g = self.ivars().git.borrow();
            let Some(st) = g.status.as_ref() else { return };
            let fetched = match (&g.fetch_error, g.last_fetch) {
                (Some(e), _) => format!("Fetch failed: {}", e.lines().last().unwrap_or("error")),
                (None, Some(t)) => format!("Fetched {}", ago(t)),
                (None, None) => "Not fetched yet".into(),
            };
            let upstream = match &st.upstream {
                Some(u) => format!("Tracking {u}: ↓{} behind, ↑{} ahead", st.behind, st.ahead),
                None => "No upstream branch".into(),
            };
            (upstream, fetched, st.behind, g.busy)
        };
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let item = |title: &str, action: Option<objc2::runtime::Sel>, enabled: bool| {
            let it = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    action,
                    &NSString::from_str(""),
                )
            };
            unsafe { it.setTarget(Some(self)) };
            it.setEnabled(enabled);
            menu.addItem(&it);
        };
        item(&upstream, None, false);
        item(&fetched, None, false);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        item("Fetch Now", Some(sel!(gitFetch:)), !busy);
        item(
            "Pull (fast-forward only)",
            Some(sel!(gitPull:)),
            !busy && behind > 0,
        );
        let h = ui.git_button.bounds().size.height;
        menu.popUpMenuPositioningItem_atLocation_inView(
            None,
            NSPoint::new(0.0, h + 4.0),
            Some(&ui.git_button),
        );
    }
}
