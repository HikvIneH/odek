# Development

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

Icons and the wordmark live in `assets/`.

## Releasing

1. Bump `version` in `Cargo.toml` and add a `## <version> — <date>` section to
   `CHANGELOG.md`; merge to `main`.
2. Tag the merge and push it: `git tag v0.3.0 && git push origin v0.3.0`.

The [release workflow](../.github/workflows/release.yml) builds and signs
`Odek.app`, publishes the GitHub release with the zip and that changelog section,
and updates the cask in [HikvIneH/homebrew-tap](https://github.com/HikvIneH/homebrew-tap).
It needs three repository secrets: `SIGNING_P12_BASE64` and
`SIGNING_P12_PASSWORD` (the code-signing certificate) and `TAP_DEPLOY_KEY` (an
SSH key with write access to the tap).

Builds are signed with a self-signed certificate for now. macOS still asks
before the first launch of a downloaded copy (the cask clears that), but keeps
the app's permissions across updates because the signature's identity stays the
same.

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

## Project layout

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
4. [performance.md](performance.md) is the memory baseline: compare against it,
   and update it when a change moves the numbers.
