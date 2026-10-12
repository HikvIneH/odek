# Changelog

## 0.4.1 — 2026-10-12

- Keyboard Shortcuts (⌘/) is larger and centred over the window, with every
  command in three columns, so nothing needs scrolling

## 0.4.0 — 2026-10-12

- Profiles: save a group (tabs, splits, each pane's folder and running
  command) and open it again with ⇧⌘O, or at launch with
  `open_at_launch = true`. Plain TOML files in `~/.config/odek/profiles`;
  see [docs/profiles.md](docs/profiles.md)
- Closing a panel (Settings, Keyboard Shortcuts, the profile picker) no longer
  quits odek while its window is minimized

## 0.3.0 — 2026-10-12

- Tells you when a new version is out: a daily check of the latest GitHub
  release, plus Odek ▸ Check for Updates… (turn the daily check off in the same
  menu)
- Faster scrolling: output that scrolls the whole screen comes in about 2.5×
  faster and plain scrolling about 1.9×, with no extra memory
- Fixed: clicking a tab in the sidebar selected a split tab further down
  instead, when the clicked tab sat above one
- Performance page rewritten with on-screen memory, throughput and how it's
  measured

## 0.2.0 — 2026-10-10

odek is now a terminal first, with the code viewer built in.

- Terminal: 24-bit colour, reflow on resize (scrollback included), find, ⌘-click
  URLs and `path:line:col`, OSC 8 links, input methods and emoji, Terminal.app's
  marks and clear commands, drag and drop of files and images
- Workspace: tabs grouped by project in a sidebar, split panes, focused-pane
  marker, tabs and splits restored on launch
- Code viewer as a pane: ⌘-click a path to open it at that line, ⌘P, ⇧⌘E
- Settings (⌘,): font, size, theme, scrollback, Option as Meta, opacity and blur
- Notifications when a background tab needs attention (off by default)
- Keyboard Shortcuts panel (⌘/)
- Lower memory: windows use 8-bit sRGB buffers
- New logo
- Install with Homebrew: `brew install --cask hikvineh/tap/odek`

## 0.1.0 — 2026-10-09

First release: a native code viewer with tree-sitter highlighting, ⌘P, VS Code
keys, Vim mode and a git status bar.
