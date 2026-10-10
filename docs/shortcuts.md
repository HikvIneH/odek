# Keyboard shortcuts and settings

## Tabs and panes

| Key | Action |
|---|---|
| ⌘T | New tab in the current folder and group |
| ⌘O | New tab in a folder you pick (a file you pick opens in the code viewer) |
| ⇧⌘N | New group |
| ⌘D / ⇧⌘D | Split right / split down |
| ⌘W / ⇧⌘W | Close pane (in the code viewer: the file, then the pane) / close tab |
| ⌘1 … ⌘8, ⌘9 | Go to tab 1 … 8, last tab |
| ⇧⌘] / ⇧⌘[ | Next / previous tab |
| ⌘] / ⌘[ | Next / previous pane |
| ⇧⌘R | Rename tab (an empty name follows the program's title) |
| ⌘B, ⇧⌘F | Toggle sidebar, search tabs |
| ⌘, | Settings |
| ⌘/ | Keyboard Shortcuts: every command and its keys (in the code viewer, ⌘/ toggles a comment) |

Right-click a tab or group for more: move a tab to another group, rename or
delete a group (its tabs are kept).

## Terminal

| Key | Action |
|---|---|
| ⌘C / ⌘V | Copy selection / paste |
| ⌘A | Select all, scrollback included |
| ⌘F, ⌘G / ⇧⌘G | Find, next / previous match |
| ⌘L | Clear to previous mark: remove the last command and its output |
| ⌃⌘L | Clear screen: move it into scrollback, prompt to the top |
| ⌘K / ⌥⌘K | Clear to start (scrollback and screen) / clear scrollback only |
| ⌘↑ / ⌘↓ | Jump to previous / next mark (each Return, or shell prompt) |
| ⌘Home / ⌘End | Scroll to top / bottom |
| ⌘PageUp / ⌘PageDown, ⌥⌘PageUp / ⌥⌘PageDown | Scroll a page / a line |
| ⇧PageUp / ⇧PageDown, ⇧Home / ⇧End | Scroll back / forward, to top / bottom |
| ⌘← / ⌘→ / ⌘⌫ | Start of line / end of line / delete line |
| ⌥← / ⌥→ / ⌥⌫ | Word left / word right / delete word |
| ⇧Return | Newline without sending (Claude Code and similar) |
| ⌘-click | Open a URL or file path |
| ⌘= / ⌘- / ⌘0 | Font size |

## Code viewer

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

## Vim mode

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

## Git

The code viewer's status bar shows the current branch. `main •  ↓2 ↑1` means
uncommitted changes (`•`), 2 commits to pull (`↓`, shown in orange) and 1 to
push (`↑`). odek runs `git fetch` in the background when a project opens and
when you switch back to the app, at most every 5 minutes; fetch never changes
your files. Click the branch for **Fetch Now** and **Pull (fast-forward only)**,
which refuses rather than merging when your branch has diverged.

## Settings

**Odek ▸ Settings…** (⌘,) changes every pane at once:

- **Font**: Automatic picks the first installed of MesloLGS NF and a few other
  Nerd Fonts, otherwise SF Mono; or choose any fixed-width font
- **Font size** (⌘= and ⌘- change one pane for the moment; ⌘0 goes back)
- **Theme**: follow the system, or always light or dark
- **Scrollback**: 1,000 to 50,000 lines per pane (8 MB per 10,000 lines at most)
- **Opacity**: 50–100 %; only the terminal background fades (text and the
  code viewer stay solid), with an optional blur of what's behind the window
- **Option sends Meta**: on, Option+key sends Esc+key as most shells and
  agents expect; off, Option types characters such as ™ and accents

Notifications for background tabs are off by default; turn them on in **View ▸ Notify When a
Background Tab Needs Attention**. Claude Code rings the terminal bell when it
finishes or needs input if its notification setting is the terminal bell.
