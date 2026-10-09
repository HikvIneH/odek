//! Branch and ahead/behind status by running the system `git` (no libgit2:
//! nothing added to the binary, and nothing resident between refreshes).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// Branch name, or `detached @ abc1234`.
    pub branch: String,
    /// e.g. `origin/main`; `None` if the branch tracks nothing.
    pub upstream: Option<String>,
    /// Local commits not on the upstream (need push).
    pub ahead: u32,
    /// Upstream commits not here yet (need pull).
    pub behind: u32,
    /// Uncommitted changes to tracked files.
    pub dirty: bool,
}

/// GUI apps get a minimal PATH, so look for git where it usually lives.
fn git_binary() -> &'static str {
    ["/opt/homebrew/bin/git", "/usr/local/bin/git", "/usr/bin/git"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap_or("git")
}

fn git(dir: &Path) -> Command {
    let mut cmd = Command::new(git_binary());
    cmd.arg("-C").arg(dir).stdin(Stdio::null());
    // Never block on a credential or passphrase prompt nobody can answer.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    cmd
}

/// Run git and return trimmed stdout, or stderr as the error.
fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = git(dir).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Top of the work tree containing `dir`, if it's in a git repo.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    run(dir, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from)
}

/// Local-only status (no network): branch, upstream counts, dirty flag.
pub fn status(root: &Path) -> Option<Status> {
    let mut branch = run(root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    if branch == "HEAD" {
        let sha = run(root, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
        branch = format!("detached @ {sha}");
    }
    let upstream = run(
        root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    )
    .ok();
    let (ahead, behind) = match &upstream {
        Some(_) => run(
            root,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        )
        .ok()
        .and_then(|s| parse_counts(&s))
        .unwrap_or((0, 0)),
        None => (0, 0),
    };
    let dirty = run(root, &["status", "--porcelain", "--untracked-files=no"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    Some(Status {
        branch,
        upstream,
        ahead,
        behind,
        dirty,
    })
}

/// `git rev-list --left-right --count` prints "<ahead>\t<behind>".
fn parse_counts(s: &str) -> Option<(u32, u32)> {
    let mut it = s.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// Download remote refs. Never touches the working tree.
pub fn fetch(root: &Path) -> Result<(), String> {
    run(root, &["fetch", "--quiet", "--no-tags"]).map(|_| ())
}

/// Fast-forward only: refuses (with git's message) rather than merging.
pub fn pull(root: &Path) -> Result<String, String> {
    run(root, &["pull", "--ff-only", "--no-rebase"])
}

impl Status {
    /// Status-bar text, e.g. "main •  ↓3 ↑1".
    pub fn label(&self) -> String {
        let mut s = self.branch.clone();
        if self.dirty {
            s.push_str(" •");
        }
        if self.upstream.is_some() && (self.behind > 0 || self.ahead > 0) {
            s.push_str("  ");
            if self.behind > 0 {
                s.push_str(&format!("↓{}", self.behind));
            }
            if self.ahead > 0 {
                if self.behind > 0 {
                    s.push(' ');
                }
                s.push_str(&format!("↑{}", self.ahead));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sh(dir: &Path, args: &[&str]) {
        let ok = git(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn counts_label_and_fast_forward_pull() {
        let base = std::env::temp_dir().join(format!("git-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (remote, a, b) = (base.join("remote.git"), base.join("a"), base.join("b"));
        fs::create_dir_all(&base).unwrap();
        sh(&base, &["init", "--bare", "-b", "main", remote.to_str().unwrap()]);
        sh(&base, &["clone", remote.to_str().unwrap(), a.to_str().unwrap()]);
        fs::write(a.join("f.txt"), "1\n").unwrap();
        sh(&a, &["add", "."]);
        sh(&a, &["commit", "-m", "one"]);
        sh(&a, &["push", "-u", "origin", "HEAD:main"]);
        sh(&base, &["clone", remote.to_str().unwrap(), b.to_str().unwrap()]);

        // a moves ahead and pushes; b doesn't know until it fetches.
        fs::write(a.join("f.txt"), "2\n").unwrap();
        sh(&a, &["commit", "-am", "two"]);
        sh(&a, &["push"]);
        assert_eq!(status(&b).unwrap().behind, 0);
        fetch(&b).unwrap();
        let st = status(&b).unwrap();
        assert_eq!((st.ahead, st.behind, st.dirty), (0, 1, false));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!(st.label(), "main  ↓1");

        // An uncommitted edit shows the dot; pull fast-forwards.
        fs::write(b.join("other.txt"), "x").unwrap();
        fs::write(b.join("f.txt"), "1\nlocal\n").unwrap();
        assert!(status(&b).unwrap().dirty);
        sh(&b, &["checkout", "--", "f.txt"]);
        pull(&b).unwrap();
        assert_eq!(status(&b).unwrap().behind, 0);
        assert_eq!(repo_root(&b.join(".")).unwrap().file_name(), b.file_name());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn label_formats() {
        let st = Status {
            branch: "dev".into(),
            upstream: Some("origin/dev".into()),
            ahead: 2,
            behind: 3,
            dirty: true,
        };
        assert_eq!(st.label(), "dev •  ↓3 ↑2");
        let st = Status {
            upstream: None,
            ahead: 0,
            behind: 0,
            dirty: false,
            ..st
        };
        assert_eq!(st.label(), "dev");
        assert_eq!(parse_counts("4\t0"), Some((4, 0)));
    }
}
