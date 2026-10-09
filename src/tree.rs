//! Lazy file tree. A directory is only read when it is first expanded, so
//! opening a huge repo costs one `read_dir` of the root.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// Matched by .gitignore: still shown (like VS Code), but dimmed.
    pub ignored: bool,
    /// `None` until the directory is first listed.
    pub children: Option<Vec<usize>>,
}

pub struct Tree {
    nodes: Vec<Node>,
}

pub const ROOT: usize = 0;

/// Never shown, whatever the ignore files say.
fn always_hidden(name: &str) -> bool {
    matches!(name, ".git" | ".DS_Store" | ".claude")
}

struct Entry {
    name: String,
    is_dir: bool,
    ignored: bool,
}

fn list_dir(dir: &Path) -> Vec<Entry> {
    let walk = |respect_ignores: bool| -> Vec<(String, bool)> {
        WalkBuilder::new(dir)
            .max_depth(Some(1))
            .hidden(false)
            .parents(true)
            .ignore(respect_ignores)
            .git_ignore(respect_ignores)
            .git_global(respect_ignores)
            .git_exclude(respect_ignores)
            .require_git(false)
            .build()
            .filter_map(Result::ok)
            .filter(|e| e.depth() == 1)
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_owned();
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    || (e.path_is_symlink() && e.path().is_dir());
                Some((name, is_dir))
            })
            .collect()
    };
    let kept: HashSet<String> = walk(true).into_iter().map(|(n, _)| n).collect();
    let mut entries: Vec<Entry> = walk(false)
        .into_iter()
        .filter(|(n, _)| !always_hidden(n))
        .map(|(name, is_dir)| Entry {
            ignored: !kept.contains(&name),
            name,
            is_dir,
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

impl Tree {
    pub fn new(root: PathBuf) -> Tree {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        Tree {
            nodes: vec![Node {
                path: root,
                name,
                is_dir: true,
                ignored: false,
                children: None,
            }],
        }
    }

    pub fn root(&self) -> &Path {
        &self.nodes[ROOT].path
    }

    pub fn node(&self, idx: usize) -> &Node {
        &self.nodes[idx]
    }

    /// Children of a directory, listing it on first access.
    pub fn children(&mut self, idx: usize) -> &[usize] {
        if self.nodes[idx].children.is_none() {
            let kids = self.load(idx, &mut HashMap::new());
            self.nodes[idx].children = Some(kids);
        }
        self.nodes[idx].children.as_deref().unwrap_or(&[])
    }

    /// Lists `idx`, reusing nodes from `reuse` (name → node) so expansion state
    /// and outline-view item identity survive a refresh.
    fn load(&mut self, idx: usize, reuse: &mut HashMap<String, usize>) -> Vec<usize> {
        if !self.nodes[idx].is_dir {
            return Vec::new();
        }
        let dir = self.nodes[idx].path.clone();
        list_dir(&dir)
            .into_iter()
            .map(|e| match reuse.remove(&e.name) {
                Some(old) if self.nodes[old].is_dir == e.is_dir => {
                    self.nodes[old].ignored = e.ignored;
                    old
                }
                _ => {
                    self.nodes.push(Node {
                        path: dir.join(&e.name),
                        name: e.name,
                        is_dir: e.is_dir,
                        ignored: e.ignored,
                        children: None,
                    });
                    self.nodes.len() - 1
                }
            })
            .collect()
    }

    /// Re-lists every directory that has been opened. Returns the directories
    /// whose contents changed, so only those rows get reloaded.
    pub fn refresh(&mut self) -> Vec<usize> {
        let mut changed = Vec::new();
        let mut queue = vec![ROOT];
        while let Some(idx) = queue.pop() {
            let Some(old) = self.nodes[idx].children.clone() else {
                continue;
            };
            let mut reuse: HashMap<String, usize> =
                old.iter().map(|&c| (self.nodes[c].name.clone(), c)).collect();
            let old_ignored: Vec<bool> = old.iter().map(|&c| self.nodes[c].ignored).collect();
            let new = self.load(idx, &mut reuse);
            let new_ignored: Vec<bool> = new.iter().map(|&c| self.nodes[c].ignored).collect();
            if new != old || new_ignored != old_ignored {
                changed.push(idx);
            }
            queue.extend(new.iter().copied().filter(|&c| self.nodes[c].is_dir));
            self.nodes[idx].children = Some(new);
        }
        changed
    }

    /// Chain of node indices from the root's child down to `path`, loading
    /// directories on the way. Used to reveal the open file in the sidebar.
    pub fn path_to(&mut self, path: &Path) -> Option<Vec<usize>> {
        let rel = path.strip_prefix(self.root()).ok()?.to_path_buf();
        let mut chain = Vec::new();
        let mut cur = ROOT;
        for part in rel.components() {
            let part = part.as_os_str().to_str()?;
            self.children(cur);
            let kids = self.nodes[cur].children.as_deref().unwrap_or(&[]);
            let next = *kids.iter().find(|&&c| self.nodes[c].name == part)?;
            chain.push(next);
            cur = next;
        }
        Some(chain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tree-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/inner")).unwrap();
        fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".gitignore"), "node_modules\n*.log\n").unwrap();
        fs::write(dir.join("b.txt"), "").unwrap();
        fs::write(dir.join("A.md"), "").unwrap();
        fs::write(dir.join("debug.log"), "").unwrap();
        fs::write(dir.join("src/main.rs"), "").unwrap();
        dir
    }

    fn names(t: &mut Tree, idx: usize) -> Vec<(String, bool)> {
        let kids = t.children(idx).to_vec();
        kids.iter()
            .map(|&c| (t.node(c).name.clone(), t.node(c).ignored))
            .collect()
    }

    #[test]
    fn dirs_first_ignored_dimmed_git_hidden() {
        let dir = scratch("list");
        let mut t = Tree::new(dir.clone());
        assert_eq!(
            names(&mut t, ROOT),
            vec![
                ("node_modules".into(), true),
                ("src".into(), false),
                (".gitignore".into(), false),
                ("A.md".into(), false),
                ("b.txt".into(), false),
                ("debug.log".into(), true),
            ]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refresh_keeps_node_identity_and_reports_changes() {
        let dir = scratch("refresh");
        let mut t = Tree::new(dir.clone());
        let src = t.path_to(&dir.join("src/main.rs")).unwrap();
        assert_eq!(src.len(), 2);
        assert!(t.refresh().is_empty());
        fs::write(dir.join("src/new.rs"), "").unwrap();
        let changed = t.refresh();
        assert_eq!(changed, vec![src[0]]);
        // main.rs keeps its node index across the refresh.
        assert_eq!(t.path_to(&dir.join("src/main.rs")).unwrap(), src);
        fs::remove_dir_all(dir).unwrap();
    }
}
