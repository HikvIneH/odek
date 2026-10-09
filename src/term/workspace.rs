//! The workspace model: groups of tabs, each tab a tree of split panes. No
//! AppKit here. Saved as a small indented text file so tabs, splits and
//! folders come back on relaunch (the shells themselves start fresh).

use std::path::PathBuf;

pub type Id = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Pane(Id),
    Split {
        /// Side by side (⌘D) rather than stacked (⇧⌘D).
        across: bool,
        /// Share of the first child, 0..1.
        ratio: f64,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    pub fn panes(&self) -> Vec<Id> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<Id>) {
        match self {
            Node::Pane(id) => out.push(*id),
            Node::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    /// Split `target` in two, with `new` after it. False if `target` isn't here.
    pub fn split(&mut self, target: Id, new: Id, across: bool) -> bool {
        match self {
            Node::Pane(id) if *id == target => {
                *self = Node::Split {
                    across,
                    ratio: 0.5,
                    first: Box::new(Node::Pane(target)),
                    second: Box::new(Node::Pane(new)),
                };
                true
            }
            Node::Pane(_) => false,
            Node::Split { first, second, .. } => first.split(target, new, across) || second.split(target, new, across),
        }
    }

    /// The tree without `target`; None when nothing is left.
    pub fn without(self, target: Id) -> Option<Node> {
        match self {
            Node::Pane(id) if id == target => None,
            Node::Pane(_) => Some(self),
            Node::Split { across, ratio, first, second } => match (first.without(target), second.without(target)) {
                (Some(a), Some(b)) => Some(Node::Split { across, ratio, first: Box::new(a), second: Box::new(b) }),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tab {
    pub id: Id,
    /// Set by renaming; otherwise the sidebar shows the focused pane's title.
    pub name: Option<String>,
    pub root: Node,
    pub focus: Id,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: Id,
    pub name: String,
    pub collapsed: bool,
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Default)]
pub struct Workspace {
    pub groups: Vec<Group>,
    /// The tab on screen.
    pub active: Option<Id>,
    next: Id,
}

/// What closing a pane did.
#[derive(Debug, PartialEq)]
pub enum Closed {
    Pane { tab: Id, focus: Id },
    Tab(Id),
    Nothing,
}

impl Workspace {
    pub fn new_id(&mut self) -> Id {
        self.next += 1;
        self.next
    }

    pub fn add_group(&mut self, name: &str) -> Id {
        let id = self.new_id();
        self.groups.push(Group { id, name: name.to_string(), collapsed: false, tabs: Vec::new() });
        id
    }

    /// Add a one-pane tab to `group`, after `after` if given, and activate it.
    pub fn add_tab(&mut self, group: Id, after: Option<Id>, pane: Id) -> Id {
        let id = self.new_id();
        let tab = Tab { id, name: None, root: Node::Pane(pane), focus: pane };
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == group) {
            let at = after.and_then(|a| g.tabs.iter().position(|t| t.id == a)).map_or(g.tabs.len(), |i| i + 1);
            g.tabs.insert(at, tab);
            g.collapsed = false;
        }
        self.active = Some(id);
        id
    }

    pub fn tab(&self, id: Id) -> Option<&Tab> {
        self.groups.iter().flat_map(|g| &g.tabs).find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: Id) -> Option<&mut Tab> {
        self.groups.iter_mut().flat_map(|g| &mut g.tabs).find(|t| t.id == id)
    }

    pub fn group_of(&self, tab: Id) -> Option<Id> {
        self.groups.iter().find(|g| g.tabs.iter().any(|t| t.id == tab)).map(|g| g.id)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tab(self.active?)
    }

    pub fn tab_of_pane(&self, pane: Id) -> Option<Id> {
        self.groups.iter().flat_map(|g| &g.tabs).find(|t| t.root.panes().contains(&pane)).map(|t| t.id)
    }

    /// All tabs in sidebar order.
    pub fn tabs(&self) -> Vec<Id> {
        self.groups.iter().flat_map(|g| g.tabs.iter().map(|t| t.id)).collect()
    }

    /// Next (or previous) tab after the active one, wrapping.
    pub fn cycle(&self, forward: bool) -> Option<Id> {
        let tabs = self.tabs();
        let i = tabs.iter().position(|&t| Some(t) == self.active)?;
        let n = tabs.len();
        Some(tabs[if forward { (i + 1) % n } else { (i + n - 1) % n }])
    }

    pub fn split(&mut self, tab: Id, new: Id, across: bool) {
        if let Some(t) = self.tab_mut(tab) {
            let target = t.focus;
            if t.root.split(target, new, across) {
                t.focus = new;
            }
        }
    }

    /// Remove a pane; its tab goes too when it was the last one. The next
    /// active tab is the neighbour in the sidebar.
    pub fn close_pane(&mut self, pane: Id) -> Closed {
        let Some(tab_id) = self.tab_of_pane(pane) else { return Closed::Nothing };
        let tab = self.tab_mut(tab_id).unwrap();
        let old = tab.root.panes();
        match tab.root.clone().without(pane) {
            Some(root) => {
                // Focus the pane that took the closed one's place.
                let i = old.iter().position(|&p| p == pane).unwrap_or(0);
                let left = root.panes();
                tab.root = root;
                if tab.focus == pane {
                    tab.focus = left[i.min(left.len() - 1)];
                }
                Closed::Pane { tab: tab_id, focus: tab.focus }
            }
            None => {
                self.remove_tab(tab_id);
                Closed::Tab(tab_id)
            }
        }
    }

    pub fn remove_tab(&mut self, tab: Id) {
        let order = self.tabs();
        for g in &mut self.groups {
            g.tabs.retain(|t| t.id != tab);
        }
        if self.active == Some(tab) {
            let i = order.iter().position(|&t| t == tab).unwrap_or(0);
            let rest: Vec<Id> = order.into_iter().filter(|&t| t != tab).collect();
            self.active = rest.get(i.min(rest.len().saturating_sub(1))).copied();
        }
    }

    /// Move a tab into `group` at `index` (clamped).
    pub fn move_tab(&mut self, tab: Id, group: Id, index: usize) {
        let Some(t) = self.tab(tab).cloned() else { return };
        let Some(gi) = self.groups.iter().position(|g| g.id == group) else { return };
        let from_same = self.groups[gi].tabs.iter().position(|x| x.id == tab);
        for g in &mut self.groups {
            g.tabs.retain(|x| x.id != tab);
        }
        let mut at = index;
        if let Some(old) = from_same
            && old < index
        {
            at -= 1;
        }
        let tabs = &mut self.groups[gi].tabs;
        tabs.insert(at.min(tabs.len()), t);
    }

    /// Delete a group; its tabs move to the neighbouring group (none if it's the last).
    pub fn remove_group(&mut self, group: Id) -> Vec<Id> {
        let Some(gi) = self.groups.iter().position(|g| g.id == group) else { return Vec::new() };
        let g = self.groups.remove(gi);
        if self.groups.is_empty() {
            let closed: Vec<Id> = g.tabs.iter().map(|t| t.id).collect();
            for &t in &closed {
                if self.active == Some(t) {
                    self.active = None;
                }
            }
            return closed;
        }
        let into = gi.min(self.groups.len() - 1);
        self.groups[into].tabs.extend(g.tabs);
        Vec::new()
    }

    // ---- saving ----

    /// Text form; `cwd` gives each pane's folder.
    pub fn save(&self, cwd: impl Fn(Id) -> Option<PathBuf>) -> String {
        let mut out = String::from("odek-workspace 1\n");
        for g in &self.groups {
            out.push_str(&format!("group {} {}\n", g.collapsed as u8, one_line(&g.name)));
            for t in &g.tabs {
                let active = (self.active == Some(t.id)) as u8;
                out.push_str(&format!("  tab {active} {}\n", t.name.as_deref().map_or(String::new(), one_line)));
                save_node(&t.root, t.focus, 2, &cwd, &mut out);
            }
        }
        out
    }

    /// Parse a saved workspace. Returns it with each new pane id's folder.
    pub fn load(text: &str) -> Option<(Workspace, Vec<(Id, PathBuf)>)> {
        let mut lines = text.lines().peekable();
        if lines.next()? != "odek-workspace 1" {
            return None;
        }
        let mut ws = Workspace::default();
        let mut panes = Vec::new();
        while let Some(line) = lines.next() {
            let line = line.trim_start();
            if let Some(rest) = line.strip_prefix("group ") {
                let (collapsed, name) = rest.split_once(' ').unwrap_or((rest, ""));
                let id = ws.add_group(name);
                ws.groups.last_mut().unwrap().collapsed = collapsed == "1";
                let _ = id;
            } else if let Some(rest) = line.strip_prefix("tab ") {
                let (active, name) = rest.split_once(' ').unwrap_or((rest, ""));
                let mut focus = None;
                let root = load_node(&mut lines, &mut ws, &mut panes, &mut focus)?;
                let id = ws.new_id();
                let first = root.panes()[0];
                let tab = Tab { id, name: (!name.is_empty()).then(|| name.to_string()), focus: focus.unwrap_or(first), root };
                ws.groups.last_mut()?.tabs.push(tab);
                if active == "1" {
                    ws.active = Some(id);
                }
            }
        }
        if ws.active.is_none() {
            ws.active = ws.tabs().first().copied();
        }
        Some((ws, panes))
    }
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

fn save_node(node: &Node, focus: Id, depth: usize, cwd: &impl Fn(Id) -> Option<PathBuf>, out: &mut String) {
    let pad = "  ".repeat(depth);
    match node {
        Node::Pane(id) => {
            let dir = cwd(*id).map(|p| p.display().to_string()).unwrap_or_default();
            out.push_str(&format!("{pad}pane {} {}\n", (*id == focus) as u8, one_line(&dir)));
        }
        Node::Split { across, ratio, first, second } => {
            out.push_str(&format!("{pad}split {} {ratio:.3}\n", if *across { "across" } else { "down" }));
            save_node(first, focus, depth + 1, cwd, out);
            save_node(second, focus, depth + 1, cwd, out);
        }
    }
}

fn load_node<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    ws: &mut Workspace,
    panes: &mut Vec<(Id, PathBuf)>,
    focus: &mut Option<Id>,
) -> Option<Node> {
    let line = lines.next()?.trim_start();
    if let Some(rest) = line.strip_prefix("pane ") {
        let (focused, dir) = rest.split_once(' ').unwrap_or((rest, ""));
        let id = ws.new_id();
        panes.push((id, PathBuf::from(dir)));
        if focused == "1" {
            *focus = Some(id);
        }
        Some(Node::Pane(id))
    } else if let Some(rest) = line.strip_prefix("split ") {
        let (dir, ratio) = rest.split_once(' ')?;
        let first = load_node(lines, ws, panes, focus)?;
        let second = load_node(lines, ws, panes, focus)?;
        Some(Node::Split {
            across: dir == "across",
            ratio: ratio.parse::<f64>().ok()?.clamp(0.1, 0.9),
            first: Box::new(first),
            second: Box::new(second),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Workspace, Id, Id) {
        let mut ws = Workspace::default();
        let work = ws.add_group("Work");
        let personal = ws.add_group("Personal");
        let p1 = ws.new_id();
        ws.add_tab(work, None, p1);
        let p2 = ws.new_id();
        let t2 = ws.add_tab(personal, None, p2);
        (ws, t2, p2)
    }

    #[test]
    fn split_and_close() {
        let (mut ws, tab, p2) = sample();
        let p3 = ws.new_id();
        ws.split(tab, p3, true);
        let p4 = ws.new_id();
        ws.split(tab, p4, false);
        assert_eq!(ws.tab(tab).unwrap().root.panes(), [p2, p3, p4]);
        assert_eq!(ws.tab(tab).unwrap().focus, p4);
        assert_eq!(ws.close_pane(p4), Closed::Pane { tab, focus: p3 });
        assert_eq!(ws.close_pane(p2), Closed::Pane { tab, focus: p3 });
        assert_eq!(ws.tab(tab).unwrap().root, Node::Pane(p3));
        assert_eq!(ws.close_pane(p3), Closed::Tab(tab));
        assert_eq!(ws.tabs().len(), 1);
        assert_eq!(ws.active, ws.tabs().first().copied(), "neighbour becomes active");
    }

    #[test]
    fn cycle_and_move() {
        let (mut ws, t2, _) = sample();
        let t1 = ws.tabs()[0];
        assert_eq!(ws.cycle(true), Some(t1));
        assert_eq!(ws.cycle(false), Some(t1));
        let work = ws.groups[0].id;
        ws.move_tab(t2, work, 0);
        assert_eq!(ws.groups[0].tabs.iter().map(|t| t.id).collect::<Vec<_>>(), [t2, t1]);
        assert!(ws.groups[1].tabs.is_empty());
        ws.move_tab(t2, work, 2);
        assert_eq!(ws.groups[0].tabs.iter().map(|t| t.id).collect::<Vec<_>>(), [t1, t2]);
    }

    #[test]
    fn remove_group_keeps_tabs() {
        let (mut ws, _, _) = sample();
        let personal = ws.groups[1].id;
        assert!(ws.remove_group(personal).is_empty());
        assert_eq!(ws.groups.len(), 1);
        assert_eq!(ws.groups[0].tabs.len(), 2);
    }

    #[test]
    fn save_and_load_round_trip() {
        let (mut ws, tab, p2) = sample();
        let p3 = ws.new_id();
        ws.split(tab, p3, true);
        ws.tab_mut(tab).unwrap().name = Some("POS machine".into());
        ws.groups[0].collapsed = true;
        let text = ws.save(|id| Some(PathBuf::from(format!("/dir/{}", if id == p2 { "a b" } else { "c" }))));
        let (back, panes) = Workspace::load(&text).unwrap();
        assert_eq!(back.groups.len(), 2);
        assert_eq!(back.groups[0].name, "Work");
        assert!(back.groups[0].collapsed);
        let t = back.active_tab().unwrap();
        assert_eq!(t.name.as_deref(), Some("POS machine"));
        let ids = t.root.panes();
        assert_eq!(ids.len(), 2);
        assert_eq!(t.focus, ids[1], "focus survives");
        assert!(matches!(t.root, Node::Split { across: true, .. }));
        let dirs: Vec<_> = panes.iter().map(|(_, d)| d.display().to_string()).collect();
        assert_eq!(dirs, ["/dir/c", "/dir/a b", "/dir/c"]);
        assert_eq!(back.save(|_| None).lines().count(), text.lines().count());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Workspace::load("nope").is_none());
        assert!(Workspace::load("odek-workspace 1\ngroup 0 G\n  tab 0 \n    split across x\n").is_none());
    }
}
