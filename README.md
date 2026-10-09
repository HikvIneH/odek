<p align="center">
  <img src="assets/icon-1024.png" width="112" alt="Odek icon">
</p>

<h1 align="center">Odek</h1>

<p align="center">
  <em>An ode to the code editor.</em><br>
  A tiny, native macOS code viewer and editor. Opens in ~140 ms, idles at ~22 MB.
</p>

<p align="center">
  <a href="https://github.com/HikvIneH/odek/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/HikvIneH/odek/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Platform: macOS 12+" src="https://img.shields.io/badge/platform-macOS%2012%2B-lightgrey.svg">
  <img alt="Language: Rust" src="https://img.shields.io/badge/language-Rust-orange.svg">
  <img alt="UI: AppKit" src="https://img.shields.io/badge/UI-AppKit-black.svg">
</p>

<p align="center">
  <img src="docs/screenshot.png" width="820" alt="Odek showing a Rust file with the file tree, syntax highlighting, git branch and Vim mode in the status bar">
</p>

## Why

Sometimes you just need to look at a project: browse the tree, open a few
files, maybe fix a line. Opening VS Code for that costs hundreds of megabytes
and a few seconds, which hurts on an 8 GB laptop. Odek is a file tree and an
editor, nothing else: no Electron, no language servers, no extensions. It keeps
VS Code's keyboard shortcuts and habits, and has an optional Vim mode.

## Features

- **File tree** that loads lazily; `.gitignore`d files shown dimmed, `.git` hidden
- **Preview tabs** like VS Code: single-click previews, double-click or typing keeps the tab
- **Quick open** (⌘P) with fuzzy matching that respects `.gitignore`
- **Syntax highlighting** with tree-sitter for Go, TypeScript/TSX, JavaScript/JSX, Rust, Swift, Python, SQL, LaTeX, Markdown, JSON, YAML, CSS, HTML and shell
- **Light and dark** themes that follow the system (GitHub palette)
- **Git status bar**: branch, commits to pull and push, uncommitted changes, background fetch, fast-forward pull
- **Vim mode** (optional): motions, operators, counts, visual mode, search, `:w` / `:q`
- **Editing basics**: line numbers, find and replace, go to line, toggle comment, auto-indent, word wrap
- **Picks up outside changes**: the tree and unmodified open files reload when you switch back to the app
- **Native**: AppKit text view, so undo, input methods, the find bar and scrolling behave like any Mac app

## Performance

Measured on an M1 MacBook (8 GB) with `scripts/selftest.sh`. Memory is the
process footprint, the number Activity Monitor shows.

| | Odek |
|---|---|
| Project open, no file | ~22 MB |
| One file open | ~50 MB |
| Five files open | ~57 MB |
| Main window on screen after launch | ~140 ms |
| App size on disk | 19 MB |

## Requirements

- macOS 12 or later (built and tested on Apple Silicon)
- Rust toolchain (`cargo`) to build from source
- `git` (the one that comes with Xcode Command Line Tools is fine) for the git status bar

## Installation

There are no prebuilt releases yet; build from source:

```sh
git clone https://github.com/HikvIneH/odek.git
cd odek
scripts/bundle.sh --install
```

This builds a release binary, wraps it in `Odek.app`, signs it ad hoc, and
installs:

- `~/Applications/Odek.app`
- `~/.local/bin/odek`, a small launcher script (make sure `~/.local/bin` is on your `PATH`)

`scripts/bundle.sh` without `--install` only builds `dist/Odek.app`.

## Usage

```sh
odek .                  # open the current folder
odek ~/code/project     # open a folder
odek src/main.go        # open a file; its folder becomes the project
```

Each `odek` call opens its own window and process, like `code .`. You can also
open a folder from **File ▸ Open Folder…** (⌘O) or drop one on the Dock icon.

### Keyboard shortcuts

| Key | Action |
|---|---|
| ⌘P | Quick open |
| ⌘O | Open folder |
| ⌘S / ⌘W | Save / close tab |
| ⌘F, ⌥⌘F | Find, replace |
| ⌘G, ⇧⌘G, ⌘E | Next match, previous match, use selection for find |
| ⌃G | Go to line (`42` or `42:7`) |
| ⌘/ | Toggle line comment |
| ⌘B | Toggle sidebar |
| ⌥Z | Toggle word wrap (on by default for Markdown and LaTeX) |
| ⌘= / ⌘- / ⌘0 | Font size |
| ⇧⌘] / ⇧⌘[, ⌃Tab / ⌃⇧Tab | Next / previous tab |
| ⌥⌘R | Reveal file in Finder |
| ⌥⌘V | Toggle Vim mode |

### Vim mode

Turn it on from **View ▸ Vim Mode** (⌥⌘V); the setting is remembered. The
status bar shows `NORMAL`, `-- INSERT --` or `-- VISUAL --`, and normal mode
uses a block cursor.

| Group | Keys |
|---|---|
| Motions | `h j k l`, `w b e`, `0 ^ $`, `gg G`, `{ }`, `%`, `f t F T ; ,`, Ctrl-d/u/f/b, counts (`5j`, `3w`) |
| Operators | `d c y > <` with any motion; `dd cc yy >> <<` |
| Editing | `x X D C Y s S`, `p P`, `r`, `J`, `~`, `u`, Ctrl-r |
| Insert | `i a I A o O`; Esc, Ctrl-[ or Ctrl-c to leave |
| Visual | `v`, `V`, then `d y c x > < J o` |
| Search | `/ ? n N * #` |
| Commands | `:w`, `:q`, `:q!`, `:wq`, `:x`, `:{line}` |
| Tabs, view | `gt` / `gT`, `zz zt zb` |

Yanks also go to the macOS clipboard, and `p` pastes text copied in other apps.
Not supported yet: `.` repeat, text objects (`iw`, `a(`), marks, macros and
registers other than the unnamed one.

### Git

The status bar shows the current branch, like VS Code. `main •  ↓2 ↑1` means
uncommitted changes (`•`), 2 commits to pull (`↓`, shown in orange) and 1 to
push (`↑`).

- Odek runs `git fetch` in the background when a project opens and when you
  switch back to the app, at most every 5 minutes. Fetch never changes your
  files.
- Click the branch for **Fetch Now** and **Pull (fast-forward only)**. Pull
  refuses rather than merging when your branch has diverged, and asks you to
  save open edits first.

It runs the system `git`, so it adds nothing to the app's size or resident
memory.

## How it works

- **Lazy tree**: a folder is read only when you expand it.
- **At most 8 tabs** stay in memory; opening a 9th closes the least recently
  used tab without unsaved changes.
- **Big files**: over 2 MB opens without highlighting; over 20 MB opens the
  first 5 MB read-only; binary files aren't loaded.
- **Highlighting**: tree-sitter grammars are compiled in, but each is set up
  only when a file of that language is first opened. Colours are applied as
  temporary layout attributes, so recolouring never re-lays out text or touches
  undo.
- **Text view**: TextKit 1 `NSTextView`, opted out of responsive scrolling so
  only the visible part of a document is rendered.

## Development

```sh
cargo test                    # unit tests: tree, fuzzy match, highlighting, git, Vim, edits
cargo run -- ~/some/project   # debug build
```

`scripts/selftest.sh` drives the real UI off-screen. It opens files, runs quick
open, switches to dark mode, edits, then writes PNG snapshots of the window and
the memory footprint after each step:

```sh
cargo build --release --features selftest   # the self-test is left out of normal builds
scripts/selftest.sh target/release/odek <project> <out-dir> <query> <file>...
```

Extra environment variables: `SELFTEST_NOSNAP=1` (measure memory without the
snapshot bitmaps), `SELFTEST_IDLE=12` (open files, then idle and sample),
`SELFTEST_PIN=1` (pinned tabs instead of preview), `SELFTEST_LIGHT=1`, and
`SELFTEST_VIM='keys'` (type Vim keys through real key events; `⎋` is Esc, `⏎`
is Return).

The icon ("stanza": code lines set like verse) is drawn by
`assets/make-icon.swift`, with simpler artwork at 16 and 32 px;
`assets/make-icns.sh` rebuilds `assets/AppIcon.icns`.

### Project layout

```
src/main.rs       NSApplication setup
src/app.rs        window, tree, tabs, editor, quick open, menus (one delegate)
src/tree.rs       lazy file tree model
src/fuzzy.rs      file listing and nucleo ranking for ⌘P
src/highlight.rs  tree-sitter languages → coloured UTF-16 spans
src/theme.rs      GitHub light/dark palette as dynamic NSColors
src/ruler.rs      line-number gutter
src/codeview.rs   NSTextView subclass (block cursor, key routing)
src/edit.rs       pure text transforms (comment toggle, indent)
src/git.rs        branch, ahead/behind, fetch, pull via the git CLI
src/gitbar.rs     status-bar branch button and menu
src/vim.rs        Vim engine over an abstract buffer
src/vimglue.rs    NSTextView adapter for the Vim engine
src/selftest.rs   scripted UI test
```

## Contributing

Issues and pull requests are welcome. Before opening a PR:

1. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test`
   (CI runs the same on macOS).
2. For UI changes, run `scripts/selftest.sh` and check the snapshots.
3. Keep the footprint in mind: a feature that adds resident memory or startup
   time needs a good reason, and the PR should say how much.

## License

[MIT](LICENSE). Third-party crates and their licenses are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) (regenerate with
`python3 scripts/notices.py > THIRD_PARTY_NOTICES.md`).
