<p align="center">
  <img src="assets/odek-lockup.png" width="420" alt="odek">
</p>
<p align="center">A tiny, native terminal for macOS, made for running coding agents side by side.<br>
Grouped tabs, split panes and a built-in code viewer, in about 22 MB.</p>

<p align="center">
  <a href="https://github.com/HikvIneH/odek/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/HikvIneH/odek/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-3B82F6.svg?labelColor=0B0F14"></a>
  <img alt="Platform: macOS 12+" src="https://img.shields.io/badge/platform-macOS%2012%2B-3B82F6.svg?labelColor=0B0F14">
  <img alt="Language: Rust" src="https://img.shields.io/badge/language-Rust-3B82F6.svg?labelColor=0B0F14">
  <img alt="UI: AppKit" src="https://img.shields.io/badge/UI-AppKit-3B82F6.svg?labelColor=0B0F14">
</p>

<p align="center">
  <img src="docs/odek-window.png" width="860" alt="odek: a sidebar of tabs grouped by project, a terminal pane showing git history and files, and the code viewer showing Rust source beside it">
</p>

## Why

A working day can mean several coding agents running at once, each in its own
terminal, with a quick look at the code now and then. Terminals that draw with
their own GPU renderer and ship AI and cloud features tend to sit at several
hundred megabytes and grow into gigabytes after a long session. On an 8 GB
laptop that is memory your builds and browsers need.

odek does the everyday job with what macOS already has. AppKit and Core Text
draw every character, so there is no renderer, font engine or UI toolkit of its
own to keep in memory. A terminal cell takes 8 bytes, colours are stored once,
and scrollback has a hard cap, so memory stays flat no matter how much output a
session produces.

## Features

### Workspace

- **Tabs grouped by project** in a sidebar: search, drag to reorder or move between groups, rename, collapse
- **Status at a glance**: each tab shows its folder and a dot, green while a program runs and orange when one rang the bell or sent a notification you haven't seen
- **Notifications**: when a tab you aren't looking at needs you (an agent finished or is waiting), macOS shows a banner; click it to jump to that tab
- **Split panes**, side by side or stacked, as many as you like, with draggable dividers
- **Comes back as you left it**: groups, tabs, splits and every pane's folder are restored on relaunch
- **Asks before ending work**: closing a pane, tab or the app asks first while a program such as Claude Code is still running, or a file has unsaved changes

### Terminal

- **Runs anything**: your login shell, `vim`, `htop`, and coding agents such as Claude Code, with 24-bit colour, synchronized output (no flicker), bracketed paste, mouse reporting and focus events
- **Keys that agents expect**: Shift+Return for a newline, Shift+Tab, Option as Meta (Option+←/→ jump words), ⌘←/⌘→/⌘⌫ for line editing
- **Titles that mean something**: a program's own title while it runs (Claude Code shows its task there), the folder at a shell prompt
- **Text reflows** when a pane is resized, scrollback included
- **Unicode**: wide CJK characters, emoji with skin tones and joined sequences, combining accents, input methods and the emoji picker
- **Clean lines**: box-drawing and block characters are drawn as shapes, so borders and logos join without gaps in any font
- **Nerd Font icons**: uses MesloLGS NF (or another Nerd Font) when installed, so prompt themes like powerlevel10k show their icons
- **Scrollback** of 10,000 lines per pane, find (⌘F) with every match highlighted, selection by word or line, ⌘K to clear
- **Links**: ⌘-click URLs and file paths, including `path:line:col` and the hyperlinks Claude Code prints
- **Settings** (⌘,): font, size, light/dark/system theme, scrollback length, and whether Option sends Meta

### Code viewer

Files open in a pane beside your terminal, so you can read what an agent just
changed without leaving the window.

- **⌘-click a path** in the terminal to open it at that line, or press **⌘P** to search the files of the folder you're in; **⇧⌘E** shows the file tree
- **Syntax highlighting** with tree-sitter for Go, TypeScript/TSX, JavaScript/JSX, Rust, Swift, Python, SQL, LaTeX, Markdown, JSON, YAML, CSS, HTML and shell
- **Light editing** with VS Code's keys: save, find and replace, go to line, toggle comment, auto-indent, word wrap; an optional **Vim mode**
- **Git in the status bar**: branch, commits to pull and push, uncommitted changes, background fetch, fast-forward pull
- **Picks up outside changes**: files an agent rewrote reload when you come back to the app
- **Stays light**: at most 8 files are kept in memory, big files open without highlighting, and closing the pane frees them

## Performance

Measured on an M1 Mac with 8 GB. Memory is the process footprint, the
number Activity Monitor shows.

| | |
|---|---|
| One shell, idle | ~22 MB |
| Claude Code running in a pane | ~36 MB |
| Two tabs, three panes | ~45 MB |
| Six panes, each with a full scrollback | ~75 MB (133 MB peak while all six printed at once) |
| A file open in the code viewer beside a terminal | ~58 MB |
| `seq 1 3000000` | 3.1 s, then back to ~35 MB |
| Full scrollback (10,000 lines) | ~2 MB per pane |
| Reflowing 20,000 lines on resize | ~4 ms |
| App size on disk | 19 MB |

## Requirements

- macOS 12 or later (built and tested on Apple Silicon)
- Rust toolchain (`cargo`) to build from source
- `git` (the one from Xcode Command Line Tools is fine) for the git status bar
- Optional: a [Nerd Font](https://www.nerdfonts.com) such as MesloLGS NF for prompt icons

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
- `~/.local/bin/odek`, a small launcher (make sure `~/.local/bin` is on your `PATH`)

`scripts/bundle.sh` without `--install` only builds `dist/Odek.app`.

## Usage

```sh
odek                    # open odek; your tabs come back
odek ~/code/app         # a new tab in that folder
odek src/main.rs        # the file in the code viewer
odek --viewer ~/code    # the code viewer in a window of its own
```

`odek` hands paths to the odek that's already running, so everything stays in
one window. You can also drop a folder or file on the Dock icon.

### Tabs and panes

| Key | Action |
|---|---|
| ⌘T | New tab in the current folder and group |
| ⇧⌘N | New group |
| ⌘D / ⇧⌘D | Split right / split down |
| ⌘W / ⇧⌘W | Close pane (in the code viewer: the file, then the pane) / close tab |
| ⌘1 … ⌘8, ⌘9 | Go to tab 1 … 8, last tab |
| ⇧⌘] / ⇧⌘[ | Next / previous tab |
| ⌘] / ⌘[ | Next / previous pane |
| ⇧⌘R | Rename tab (an empty name follows the program's title) |
| ⌘B, ⇧⌘F | Toggle sidebar, search tabs |
| ⌘, | Settings |

Right-click a tab or group for more: move a tab to another group, rename or
delete a group (its tabs are kept).

### Terminal

| Key | Action |
|---|---|
| ⌘C / ⌘V | Copy selection / paste |
| ⌘A | Select all, scrollback included |
| ⌘F, ⌘G / ⇧⌘G | Find, next / previous match |
| ⌘K | Clear scrollback |
| ⇧PageUp / ⇧PageDown, ⇧Home / ⇧End | Scroll back / forward, to top / bottom |
| ⌘← / ⌘→ / ⌘⌫ | Start of line / end of line / delete line |
| ⌥← / ⌥→ / ⌥⌫ | Word left / word right / delete word |
| ⇧Return | Newline without sending (Claude Code and similar) |
| ⌘-click | Open a URL or file path |
| ⌘= / ⌘- / ⌘0 | Font size |

### Code viewer

| Key | Action |
|---|---|
| ⌘P | Quick open a file in the current project |
| ⇧⌘E | Show or hide the file tree |
| ⌘S | Save |
| ⌘F, ⌥⌘F | Find, replace |
| ⌘G / ⇧⌘G, ⌘E | Next / previous match, use selection for find |
| ⌃G | Go to line (`42` or `42:7`) |
| ⌘/ | Toggle line comment |
| ⌃Tab / ⌃⇧Tab | Next / previous file |
| ⌥Z | Toggle word wrap (on by default for Markdown and LaTeX) |
| ⌥⌘V | Toggle Vim mode |
| ⌥⌘R | Reveal the file in Finder |

The project is the nearest git repository around the file or folder, so ⌘P in
a terminal inside `~/code/app/src` searches all of `~/code/app`.

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

The code viewer's status bar shows the current branch. `main •  ↓2 ↑1` means
uncommitted changes (`•`), 2 commits to pull (`↓`, shown in orange) and 1 to
push (`↑`). odek runs `git fetch` in the background when a project opens and
when you switch back to the app, at most every 5 minutes; fetch never changes
your files. Click the branch for **Fetch Now** and **Pull (fast-forward only)**,
which refuses rather than merging when your branch has diverged.

### Settings

**Odek ▸ Settings…** (⌘,) changes every pane at once:

- **Font**: Automatic picks the first installed of MesloLGS NF and a few other
  Nerd Fonts, otherwise SF Mono; or choose any fixed-width font
- **Font size** (⌘= and ⌘- change one pane for the moment; ⌘0 goes back)
- **Theme**: follow the system, or always light or dark
- **Scrollback**: 1,000 to 50,000 lines per pane (8 MB per 10,000 lines at most)
- **Option sends Meta**: on, Option+key sends Esc+key as most shells and
  agents expect; off, Option types characters such as ™ and accents

Notifications for background tabs can be turned off in **View ▸ Notify When a
Background Tab Needs Attention**. Claude Code rings the terminal bell when it
finishes or needs input if its notification setting is the terminal bell.

## How it works

Terminal:

- **Parsing**: the [`vte`](https://crates.io/crates/vte) crate (the parser
  Alacritty uses) feeds odek's own screen model.
- **8-byte cells**: each cell holds the character, a style index and flags.
  Colours, attributes and hyperlinks are interned once per pane, and emoji
  sequences live in a small shared table.
- **Capped scrollback**: lines leaving the screen are trimmed of trailing
  blanks and kept up to 10,000 lines or 8 MB, whichever comes first. Resizing
  re-wraps soft-wrapped lines rather than cutting them.
- **Drawing**: AppKit string drawing (Core Text underneath) in a plain
  `NSView`, redrawing only the rows that changed; font fallback for emoji and
  CJK comes from the system.
- **One pty per pane** with a reader thread and a writer thread, so a large
  paste never blocks the window. Updates are batched into one redraw per frame,
  and a synchronized update is shown only once it is complete.
- **No shell hooks**: a pane's folder and program come from the operating
  system (`proc_pidinfo`), not from scripts injected into your shell.

Workspace:

- A small model of groups, tabs and split trees, saved as an indented text file
  in `~/Library/Application Support/Odek/workspace.txt`.
- Tabs you aren't looking at keep running but are taken out of the window, so
  they cost no drawing.

Code viewer:

- One viewer, created the first time you open a file and moved to whichever tab
  asks for it. It is an `NSViewController`, so menu commands reach it through
  AppKit's responder chain whenever it has focus.
- TextKit 1 `NSTextView`, opted out of responsive scrolling so only the visible
  part of a file is rendered. Highlighting colours are temporary layout
  attributes, so recolouring never re-lays out text or touches undo.
- Tree-sitter grammars are compiled in, but each is set up only when a file of
  that language is first opened. Over 2 MB opens without highlighting; over
  20 MB opens the first 5 MB read-only; binary files aren't loaded.

## Development

```sh
cargo test                 # unit tests: terminal, workspace, find, links, viewer, git, Vim
cargo run                  # debug build
cargo run -- --viewer .    # the code viewer on its own
```

Both halves can be driven off-screen, writing PNG snapshots and memory numbers,
so UI changes can be checked without screen recording:

```sh
cargo build --release --features selftest   # the self-tests are left out of normal builds

# a terminal pane: one step per line (wait, keys, resize, snap, mem, find, quit, …)
scripts/termsnap.sh <out-dir> <start-dir> @steps.txt ['<command>']

# the whole window (tabs, splits, the code viewer), with a throwaway save file
ODEK_TERM_WS=1 ODEK_WORKSPACE_FILE=/tmp/ws.txt scripts/termsnap.sh <out-dir> <start-dir> @steps.txt

# the code viewer on its own
scripts/selftest.sh target/release/odek <project> <out-dir> <query> <file>...
```

`scripts/install-term-preview.sh` installs a build as a separate
`Odek Terminal Preview.app`, to try a branch without replacing `Odek.app`.

### Brand

The mark is a prompt chevron and a blue cursor on an ink tile; the wordmark is
`odek` in IBM Plex Mono. Colours: ink `#0B0F14`, foreground `#E6EDF3`, cursor
blue `#3B82F6`. The terminal's dark theme and cursor use the same colours.
Assets in `assets/`: `icon-1024.png` and `AppIcon.icns` (app icon),
`odek-icon.svg`, `odek-mark.svg` (no tile), `odek-lockup.png` and
`odek-lockup.svg` / `odek-lockup-light.svg` (wordmark; the SVGs need IBM Plex
Mono installed), and `docs/social-preview.png` for link previews.

The icon is drawn by
`assets/make-icon.swift`; `assets/make-icns.sh` builds `assets/AppIcon.icns`
from `assets/icon-1024.png` (`--redraw` regenerates that first). The mark and
the wordmark are also in `assets/` as SVG (`odek-icon`, `odek-mark`,
`odek-lockup`, `odek-lockup-light`).

### Project layout

```
src/main.rs            entry point: the terminal, or --viewer
src/term/app.rs        app delegate, menus, scripted snapshot mode
src/term/settings.rs   the settings window
src/term/notify.rs     notifications for background tabs
src/term/window.rs     workspace window: sidebar, panes, code viewer pane, dialogs
src/term/workspace.rs  groups, tabs, split trees; save and restore
src/term/sidebar.rs    grouped tab list; header.rs is the pane title strip
src/term/vt.rs         terminal state machine (escape sequences → screen), reflow
src/term/grid.rs       cells, interned styles, capped scrollback
src/term/session.rs    pty, shell process, reader and writer threads
src/term/view.rs       terminal view: drawing, keys, mouse, selection
src/term/input.rs      key, paste and mouse encoding
src/term/ime.rs        input methods (marked text, emoji picker)
src/term/find.rs       search in scrollback; findbar.rs is its UI
src/term/links.rs      URL and file path detection
src/term/boxdraw.rs    box-drawing and block characters as shapes
src/app.rs             code viewer: tree, tabs, editor, quick open, embedding
src/tree.rs            lazy file tree model
src/fuzzy.rs           file listing and nucleo ranking for ⌘P
src/highlight.rs       tree-sitter languages → coloured UTF-16 spans
src/theme.rs           GitHub light/dark palette as dynamic NSColors
src/ruler.rs           line-number gutter
src/codeview.rs        text view subclass (block cursor, key routing, focus)
src/edit.rs            pure text transforms (comment toggle, indent)
src/git.rs             branch, ahead/behind, fetch, pull via the git CLI
src/gitbar.rs          status-bar branch button and menu
src/vim.rs             Vim engine over an abstract buffer
src/vimglue.rs         text view adapter for the Vim engine
src/selftest.rs        scripted code viewer test
```

## Contributing

Issues and pull requests are welcome. Before opening a PR:

1. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` (with and
   without `--features selftest`) and `cargo test`; CI runs the same on macOS.
2. For UI changes, run `scripts/termsnap.sh` or `scripts/selftest.sh` and check
   the snapshots.
3. Keep the footprint in mind: a feature that adds resident memory or startup
   time needs a good reason, and the PR should say how much.

## License

[MIT](LICENSE). Third-party crates and their licenses are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) (regenerate with
`python3 scripts/notices.py > THIRD_PARTY_NOTICES.md`).
