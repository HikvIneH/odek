//! Profiles: saved groups (tabs, splits, a folder and an optional command per
//! pane) that ⇧⌘O opens as a new group, and that can open when odek starts.
//! Each is a TOML file in ~/.config/odek/profiles, read only while the picker
//! is open or at launch, so profiles cost nothing while odek runs. No AppKit.
//!
//! ```toml
//! name = "My app"
//! open_at_launch = true
//!
//! [[tabs]]
//! split = "across"            # side by side; "down" stacks
//! panes = [
//!   { dir = "~/code/app" },
//!   { dir = "~/code/app", run = "npm run dev" },
//! ]
//!
//! [[tabs]]
//! name = "notes"
//! dir = "~/notes"             # a one-pane tab
//! ```
//!
//! A pane can itself be a split (`{ split = "down", panes = [...] }`), and
//! `sizes = [0.6, 0.4]` gives a split's shares (even when left out).

use std::path::{Path, PathBuf};

use serde::Deserialize;

const EXT: &str = "toml";

/// A tab's layout, as odek holds it: the same binary tree as `Node`.
#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    Pane {
        /// As written: `~/…` for folders under home.
        dir: String,
        /// Typed into the pane's shell once it starts.
        run: Option<String>,
        focused: bool,
    },
    Split {
        across: bool,
        /// Share of the first child.
        ratio: f64,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProfileTab {
    pub name: Option<String>,
    pub layout: Layout,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub open_at_launch: bool,
    pub tabs: Vec<ProfileTab>,
}

/// One entry as written in the file: a tab, a pane, or a split of either.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Item {
    name: Option<String>,
    dir: Option<String>,
    run: Option<String>,
    #[serde(default)]
    focus: bool,
    split: Option<String>,
    sizes: Option<Vec<f64>>,
    #[serde(default)]
    panes: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: Option<String>,
    #[serde(default)]
    open_at_launch: bool,
    #[serde(default)]
    tabs: Vec<Item>,
}

impl Profile {
    /// Parse a profile file; `stem` names it when the file has no `name`.
    pub fn parse(text: &str, stem: &str) -> Result<Profile, String> {
        let file: File = toml::from_str(text).map_err(|e| e.message().to_string())?;
        let name = file.name.unwrap_or_else(|| stem.to_string()).trim().to_string();
        if name.is_empty() {
            return Err("the profile has no name".into());
        }
        if file.tabs.is_empty() {
            return Err("the profile has no [[tabs]]".into());
        }
        let tabs = file
            .tabs
            .into_iter()
            .map(|t| {
                Ok(ProfileTab {
                    name: t
                        .name
                        .clone()
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty()),
                    layout: layout_of(t)?,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Profile {
            name,
            open_at_launch: file.open_at_launch,
            tabs,
        })
    }

    /// The profile as TOML, written compactly (panes as inline tables).
    pub fn to_toml(&self) -> String {
        let mut out = format!("name = {}\n", quote(&self.name));
        if self.open_at_launch {
            out.push_str("open_at_launch = true\n");
        }
        for tab in &self.tabs {
            out.push_str("\n[[tabs]]\n");
            if let Some(n) = &tab.name {
                out.push_str(&format!("name = {}\n", quote(n)));
            }
            match &tab.layout {
                Layout::Pane { dir, run, .. } => {
                    out.push_str(&format!("dir = {}\n", quote(dir)));
                    if let Some(r) = run {
                        out.push_str(&format!("run = {}\n", quote(r)));
                    }
                }
                split => {
                    let (across, items, sizes) = flatten(split);
                    out.push_str(&format!(
                        "split = \"{}\"\n",
                        if across { "across" } else { "down" }
                    ));
                    if let Some(s) = sizes_field(&sizes) {
                        out.push_str(&format!("sizes = {s}\n"));
                    }
                    out.push_str("panes = [\n");
                    for item in items {
                        out.push_str(&format!("  {},\n", inline(item)));
                    }
                    out.push_str("]\n");
                }
            }
        }
        out
    }

    /// What opening it gives, one line per tab and pane, for the picker.
    pub fn preview(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, tab) in self.tabs.iter().enumerate() {
            let title = tab.name.clone().unwrap_or_else(|| format!("Tab {}", i + 1));
            let shape = match &tab.layout {
                Layout::Pane { .. } => String::new(),
                Layout::Split { .. } => format!("  ({} panes)", count_panes(&tab.layout)),
            };
            out.push(format!("{title}{shape}"));
            preview_layout(&tab.layout, &mut out);
        }
        out
    }
}

/// An entry from the file as a layout: a pane, or its panes split evenly
/// (or by `sizes`) and chained into the binary tree odek uses.
fn layout_of(item: Item) -> Result<Layout, String> {
    if item.panes.is_empty() {
        if item.split.is_some() || item.sizes.is_some() {
            return Err("a split needs panes = [...]".into());
        }
        return Ok(Layout::Pane {
            dir: item.dir.unwrap_or_else(|| "~".into()).trim().to_string(),
            run: item.run.map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
            focused: item.focus,
        });
    }
    if item.dir.is_some() || item.run.is_some() {
        return Err("an entry has either panes = [...] or dir/run, not both".into());
    }
    let across = match item.split.as_deref() {
        None | Some("across") => true,
        Some("down") => false,
        Some(other) => return Err(format!("split = \"{other}\": use \"across\" or \"down\"")),
    };
    let n = item.panes.len();
    let sizes = match item.sizes {
        Some(s) if s.len() != n => return Err(format!("sizes has {} entries for {n} panes", s.len())),
        Some(s) if s.iter().any(|v| !v.is_finite() || *v <= 0.0) => {
            return Err("sizes must be positive numbers".into());
        }
        Some(s) => s,
        None => vec![1.0; n],
    };
    let layouts = item
        .panes
        .into_iter()
        .map(layout_of)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(chain(across, layouts, &sizes))
}

/// Panes `[a, b, c]` with shares `[sa, sb, sc]` as Split(a, Split(b, c)).
fn chain(across: bool, mut layouts: Vec<Layout>, sizes: &[f64]) -> Layout {
    if layouts.len() == 1 {
        return layouts.remove(0);
    }
    let first = layouts.remove(0);
    let total: f64 = sizes.iter().sum();
    Layout::Split {
        across,
        ratio: (sizes[0] / total).clamp(0.1, 0.9),
        first: Box::new(first),
        second: Box::new(chain(across, layouts, &sizes[1..])),
    }
}

/// The reverse of `chain`: a split's run of same-direction panes and their
/// shares, so a saved profile reads as one list.
fn flatten(layout: &Layout) -> (bool, Vec<&Layout>, Vec<f64>) {
    let mut items = Vec::new();
    let mut sizes = Vec::new();
    let Layout::Split { across, .. } = layout else {
        return (true, vec![layout], vec![1.0]);
    };
    let mut rest = 1.0;
    let mut node = layout;
    while let Layout::Split {
        across: a,
        ratio,
        first,
        second,
    } = node
        && a == across
    {
        items.push(&**first);
        sizes.push(rest * ratio);
        rest *= 1.0 - ratio;
        node = second;
    }
    items.push(node);
    sizes.push(rest);
    (*across, items, sizes)
}

/// `sizes = [...]` unless the shares are even.
fn sizes_field(sizes: &[f64]) -> Option<String> {
    let even = 1.0 / sizes.len() as f64;
    if sizes.iter().all(|s| (s - even).abs() < 0.01) {
        return None;
    }
    let list: Vec<String> = sizes
        .iter()
        .map(|s| format!("{}", (s * 100.0).round() / 100.0))
        .collect();
    Some(format!("[{}]", list.join(", ")))
}

/// One entry of a `panes` list as an inline table.
fn inline(layout: &Layout) -> String {
    match layout {
        Layout::Pane { dir, run, focused } => {
            let mut fields = vec![format!("dir = {}", quote(dir))];
            if let Some(r) = run {
                fields.push(format!("run = {}", quote(r)));
            }
            if *focused {
                fields.push("focus = true".into());
            }
            format!("{{ {} }}", fields.join(", "))
        }
        split => {
            let (across, items, sizes) = flatten(split);
            let mut fields = vec![format!("split = \"{}\"", if across { "across" } else { "down" })];
            if let Some(s) = sizes_field(&sizes) {
                fields.push(format!("sizes = {s}"));
            }
            let panes: Vec<String> = items.into_iter().map(inline).collect();
            fields.push(format!("panes = [{}]", panes.join(", ")));
            format!("{{ {} }}", fields.join(", "))
        }
    }
}

/// A TOML basic string.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn preview_layout(layout: &Layout, out: &mut Vec<String>) {
    match layout {
        Layout::Pane { dir, run, .. } => {
            out.push(format!("  {}", tilde(dir)));
            if let Some(cmd) = run {
                out.push(format!("    ↳ {cmd}"));
            }
        }
        Layout::Split { first, second, .. } => {
            preview_layout(first, out);
            preview_layout(second, out);
        }
    }
}

fn count_panes(layout: &Layout) -> usize {
    match layout {
        Layout::Pane { .. } => 1,
        Layout::Split { first, second, .. } => count_panes(first) + count_panes(second),
    }
}

/// A folder for showing: under home it reads `~/…`.
fn tilde(dir: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match dir.strip_prefix(&home) {
        Some("") if !home.is_empty() => "~".into(),
        Some(rest) if !home.is_empty() && rest.starts_with('/') => format!("~{rest}"),
        _ => dir.to_string(),
    }
}

/// `~` and `~/…` to the home folder.
pub fn expand(dir: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    match dir {
        "" | "~" => home,
        _ => match dir.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(dir),
        },
    }
}

/// A command line from its arguments, quoted for the shell where needed.
pub fn command_line(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            let plain = !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%^".contains(c));
            if plain {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where profiles live: $ODEK_PROFILES_DIR, else $XDG_CONFIG_HOME/odek/profiles,
/// else ~/.config/odek/profiles.
pub fn folder() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ODEK_PROFILES_DIR") {
        return Some(PathBuf::from(p));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("odek/profiles"))
}

/// Every profile in `dir` by name, and the files that couldn't be read with why.
pub fn load_all(dir: &Path) -> (Vec<Profile>, Vec<(String, String)>) {
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (profiles, errors);
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == EXT))
        .collect();
    paths.sort();
    for path in paths {
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| Profile::parse(&t, &stem))
        {
            Ok(p) => profiles.push(p),
            Err(e) => errors.push((file, e)),
        }
    }
    profiles.sort_by_key(|p| p.name.to_lowercase());
    profiles.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    (profiles, errors)
}

/// Save `profile` as `<name>.toml`, replacing any profile with the same name.
pub fn save(dir: &Path, profile: &Profile) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.{EXT}", file_stem(&profile.name)));
    // The same name in another file (renamed by hand): replace that one.
    if let Ok(entries) = std::fs::read_dir(dir) {
        for p in entries.flatten().map(|e| e.path()) {
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if p != path
                && p.extension().is_some_and(|e| e == EXT)
                && let Some(old) = std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|t| Profile::parse(&t, &stem).ok())
                && old.name.eq_ignore_ascii_case(&profile.name)
            {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    std::fs::write(&path, profile.to_toml())?;
    Ok(path)
}

fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let stem = stem.trim().trim_start_matches('.');
    if stem.is_empty() {
        "profile".into()
    } else {
        stem.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(dir: &str, run: Option<&str>, focused: bool) -> Layout {
        Layout::Pane {
            dir: dir.into(),
            run: run.map(Into::into),
            focused,
        }
    }

    fn sample() -> Profile {
        Profile {
            name: "Work".into(),
            open_at_launch: true,
            tabs: vec![
                ProfileTab {
                    name: None,
                    // Left 60%, right stacked in two.
                    layout: Layout::Split {
                        across: true,
                        ratio: 0.6,
                        first: Box::new(pane("~/code/app", None, false)),
                        second: Box::new(Layout::Split {
                            across: false,
                            ratio: 0.5,
                            first: Box::new(pane("~/code/app", Some("npm run dev"), true)),
                            second: Box::new(pane("/tmp/with \"quotes\"", None, false)),
                        }),
                    },
                },
                ProfileTab {
                    name: Some("notes".into()),
                    layout: pane("~/notes", Some("vim todo.md"), false),
                },
            ],
        }
    }

    #[test]
    fn writes_readable_toml_that_reads_back() {
        let p = sample();
        let text = p.to_toml();
        assert_eq!(
            text,
            "name = \"Work\"\nopen_at_launch = true\n\
             \n[[tabs]]\nsplit = \"across\"\nsizes = [0.6, 0.4]\npanes = [\n  { dir = \"~/code/app\" },\n  \
             { split = \"down\", panes = [{ dir = \"~/code/app\", run = \"npm run dev\", focus = true }, \
             { dir = \"/tmp/with \\\"quotes\\\"\" }] },\n]\n\
             \n[[tabs]]\nname = \"notes\"\ndir = \"~/notes\"\nrun = \"vim todo.md\"\n"
        );
        assert_eq!(Profile::parse(&text, "x"), Ok(p));
    }

    #[test]
    fn reads_hand_written_profiles() {
        let text = r#"
            # Three side by side, the middle one wider; no name: the file's is used.
            [[tabs]]
            split = "across"
            sizes = [1, 2, 1]
            panes = [{ dir = "~/a" }, { dir = "~/b", run = "make watch" }, {}]

            [[tabs]]
            panes = [{ dir = "~/c" }, { dir = "~/d" }]
        "#;
        let p = Profile::parse(text, "side project").unwrap();
        assert_eq!(p.name, "side project");
        assert!(!p.open_at_launch);
        let Layout::Split { ratio, second, .. } = &p.tabs[0].layout else {
            panic!()
        };
        assert_eq!(*ratio, 0.25);
        let Layout::Split {
            ratio, first, second, ..
        } = &**second
        else {
            panic!()
        };
        assert!((ratio - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(**first, pane("~/b", Some("make watch"), false));
        assert_eq!(**second, pane("~", None, false));
        // Even shares and "across" when left out.
        assert!(matches!(
            p.tabs[1].layout,
            Layout::Split {
                across: true,
                ratio: 0.5,
                ..
            }
        ));
    }

    #[test]
    fn explains_what_is_wrong() {
        for (bad, why) in [
            ("[[tabs]]\ndir = \"~\"\nwat = 1", "unknown field"),
            ("name = \"x\"", "no [[tabs]]"),
            ("name = \"  \"\n[[tabs]]\ndir = \"~\"", "no name"),
            (
                "[[tabs]]\nsplit = \"sideways\"\npanes = [{}, {}]",
                "\"across\" or \"down\"",
            ),
            ("[[tabs]]\nsizes = [1]\npanes = [{}, {}]", "1 entries for 2 panes"),
            ("[[tabs]]\nsizes = [1, -1]\npanes = [{}, {}]", "positive"),
            ("[[tabs]]\ndir = \"~\"\npanes = [{}]", "not both"),
            ("[[tabs]]\nsplit = \"down\"", "needs panes"),
            ("[[tabs]\n", "invalid"),
        ] {
            let err = Profile::parse(bad, "f").unwrap_err();
            assert!(err.contains(why), "{bad:?}: {err}");
        }
    }

    #[test]
    fn previews_tabs_panes_and_commands() {
        assert_eq!(
            sample().preview(),
            [
                "Tab 1  (3 panes)",
                "  ~/code/app",
                "  ~/code/app",
                "    ↳ npm run dev",
                "  /tmp/with \"quotes\"",
                "notes",
                "  ~/notes",
                "    ↳ vim todo.md",
            ]
        );
    }

    #[test]
    fn quotes_arguments_only_when_needed() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(command_line(&args(&["npm", "run", "dev"])), "npm run dev");
        assert_eq!(
            command_line(&args(&["echo", "it's here", ""])),
            r"echo 'it'\''s here' ''"
        );
    }

    #[test]
    fn expands_home() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let h = home.display().to_string();
        assert_eq!(tilde(&format!("{h}/a")), "~/a");
        assert_eq!(tilde(&h), "~");
        assert_eq!(
            tilde(&format!("{h}x/a")),
            format!("{h}x/a"),
            "not a folder under home"
        );
        assert_eq!(expand("~"), home);
        assert_eq!(expand("~/a/b"), home.join("a/b"));
        assert_eq!(expand("/etc"), PathBuf::from("/etc"));
    }

    #[test]
    fn saves_lists_and_replaces_by_name() {
        let dir = std::env::temp_dir().join(format!("odek-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut p = sample();
        let path = save(&dir, &p).unwrap();
        assert_eq!(path.file_name().unwrap(), "Work.toml");
        // A hand-renamed file with the same profile name is replaced on save.
        std::fs::rename(&path, dir.join("old name.toml")).unwrap();
        p.tabs.truncate(1);
        save(&dir, &p).unwrap();
        let mut other = sample();
        other.name = "a/b: c".into();
        assert_eq!(save(&dir, &other).unwrap().file_name().unwrap(), "a-b- c.toml");
        std::fs::write(dir.join("broken.toml"), "[[tabs]\n").unwrap();
        std::fs::write(dir.join("notes.md"), "name = \"x\"\n[[tabs]]\ndir = \"~\"\n").unwrap();
        let (all, errors) = load_all(&dir);
        assert_eq!(
            all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["a/b: c", "Work"]
        );
        assert_eq!(all[1].tabs.len(), 1, "the newer save won");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "broken.toml");
        assert!(!dir.join("old name.toml").exists());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_all(&dir).0.is_empty());
    }
}
