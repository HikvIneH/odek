# Profiles

A profile is a saved group: its tabs, how they're split, the folder each pane
starts in and, optionally, a command to run there. **⇧⌘O** (Shell ▸ Open
Profile…) shows your profiles with a preview of what each one opens; Return
opens it as a new group. Profiles can also open by themselves when odek starts.

Profiles are TOML files in `~/.config/odek/profiles/`, one per profile, so you
can write them by hand, keep them in your dotfiles and share them.

## Make one

The quickest way is to save a group you've set up:

1. Set up a group the way you like it: ⇧⌘N, tabs, splits, `cd` into the
   folders, start the programs you want (an agent, a dev server, a watcher).
2. Right-click the group in the sidebar and choose **Save Group as Profile…**
   (or Shell ▸ Save Group as Profile… for the group of the tab you're in).
3. Give it a name.

odek saves each pane's current folder and the program running in it, if any,
with its arguments, so a pane running `npm run dev` comes back running it.
Saving under an existing name replaces that profile.

**Shell ▸ Edit Profiles…** opens the folder in the code viewer, to change or
delete profiles, or to write one from scratch.

## Open one

| Key | In the picker |
|---|---|
| ⇧⌘O | Show the picker |
| type | Filter by name |
| ↑ / ↓ | Choose |
| ↩ | Open it, or go to its group if it's already open |
| ⌥↩ | Open another copy, even if it's already open |
| esc | Close |

Each pane starts its shell in the profile's folder (your home folder if that
folder doesn't exist), and its `run` command is typed into that shell, so the
shell is still there when the command ends.

## The format

```toml
# ~/.config/odek/profiles/my-app.toml
name = "My app"           # shown in the picker and as the group's name
open_at_launch = true     # optional: open when odek starts

# Each [[tabs]] is a tab. Its panes are split side by side.
[[tabs]]
panes = [
  { dir = "~/code/app" },
  { dir = "~/code/app", run = "npm run dev" },
]

# A one-pane tab can say dir and run directly, and a name for the sidebar.
[[tabs]]
name = "api"
dir = "~/code/api"
run = "cargo watch -x run"
```

| Field | Where | Meaning |
|---|---|---|
| `name` | top | The profile's and the group's name. Left out, the file's name is used. |
| `open_at_launch` | top | `true`: opens when odek starts (see below). Default `false`. |
| `[[tabs]]` | top | One per tab, in order. At least one. |
| `name` | tab | The tab's name in the sidebar. Left out, it follows the running program's title. |
| `dir` | tab or pane | The folder to start in. `~` is your home folder. Default `~`. |
| `run` | tab or pane | A command typed into the shell once it starts. |
| `focus` | pane | `true` on the pane that gets the cursor. Default: the first. |
| `panes` | tab or pane | Split into these panes. |
| `split` | tab or pane, with `panes` | `"across"` (side by side, the default) or `"down"` (stacked). |
| `sizes` | tab or pane, with `panes` | Each pane's share, in the same order, e.g. `[2, 1]` or `[0.6, 0.4]`. Default: even. |

A pane in `panes` can itself be split, for layouts like one pane on the left
and two stacked on the right:

```toml
[[tabs]]
sizes = [0.6, 0.4]
panes = [
  { dir = "~/code/app" },
  { split = "down", panes = [
      { dir = "~/code/app", run = "npm test -- --watch" },
      { dir = "~/code/app", focus = true },
  ] },
]
```

A file that can't be read is listed at the bottom of the picker with the reason
(a misspelt field, a missing `]`, a split without panes, …); the other profiles
still work.

## At launch

When odek starts, it restores your groups and tabs from last time, then opens
every profile with `open_at_launch = true` whose group isn't already there, so
a profile never opens twice. The tab you were on stays on screen.

## Where profiles live

`~/.config/odek/profiles/*.toml`, or `$XDG_CONFIG_HOME/odek/profiles` when
`XDG_CONFIG_HOME` is set.

## Memory

Profiles cost nothing while odek runs: they're read when the picker opens (and
once at launch) and dropped when it closes. The picker itself is created when
you press ⇧⌘O and released when it closes.
