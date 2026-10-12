# Performance

Measured on an M1 MacBook with 8 GB, macOS 26, the built-in display, the power
adapter connected and nothing heavy running alongside. Memory is the process
footprint, the number Activity Monitor shows. Every figure is the median of
three runs; how they were taken is at the end.

## Memory

**What Activity Monitor shows** includes the window's drawing buffers, which
macOS keeps for any app with a window on screen. Each one is the size of the
terminal area: 21.7 MB in a window maximized on a 1680×1020-point screen.
macOS keeps one while nothing changes on screen and two to four while
something keeps redrawing, such as an agent's spinner or a progress bar. A few
seconds after the redrawing stops it's back to one.

In a window maximized on that screen:

| | Footprint | Window buffers in it |
|---|---|---|
| One idle shell | 40 MB | 22 MB |
| Two panes of agent-like output (a status block redrawn ten times a second) | 89 MB | 43 MB |
| The same, a few seconds after the output stops | 44 MB | 22 MB |
| Six panes, each with a full scrollback of real coloured output | 80 MB | 23 MB |
| A file open in the code viewer beside a terminal | 55 MB | 24 MB |

**odek's own data** is small next to that: about 6 MB for one idle shell, and
about 4–5 MB per pane with a full scrollback (10,000 lines of typical coloured
output, such as diffs and logs). Scrollback has a hard cap per pane.

odek draws its windows in sRGB. Left to itself, macOS gives windows on a
wide-colour display half-float buffers, twice the size (about 43 MB each for a
maximized window), and keeps two or three of them while anything animates. That
alone took the same window to around 120 MB idle and 200–250 MB under load.

Window opacity and blur don't change these numbers. Programs you run (a shell,
Claude Code, `vim`) are processes of their own and use the same memory in any
terminal; they aren't counted above.

**Known costs**, still to improve:

- While odek is hidden (⌘H) and a program keeps redrawing, macOS keeps up to
  six buffers (about 120 MB maximized): odek doesn't pause drawing when its
  window can't be seen yet.
- A program that animates the window title uses about 6% CPU even when it
  prints nothing, because each title change redraws the sidebar.

## Throughput

How fast odek takes in output, measured inside odek in a window 124 columns ×
44 rows. Lower is faster.

vtebench, milliseconds per 1 MiB of output:

| Payload | ms |
|---|---|
| Unicode text | 14 |
| Dense cells (every cell styled) | 18 |
| Synchronized updates of medium cells | 25 |
| Medium cells | 27 |
| Scrolling a region at the bottom (small or large) | 44 |
| Scrolling a small region at the top | 44 |
| Scrolling a large region at the top | 48 |
| Scrolling | 61 |
| Scrolling the full screen | 72 |
| **All ten** | **397** |

termbench V2, seconds:

| Test | Small (about 1 MB) | Regular (1 GiB of text) |
|---|---|---|
| ManyLine (short lines) | 2.5 | 39.3 |
| LongLine | 2.3 | 36.0 |
| FGPerChar (a colour per character) | 0.35 | 5.3 |
| FGBGPerChar (both colours per character) | 0.68 | 10.8 |
| **Total** | **5.8** | **91.3** |

Text comes in at about 25 MB/s on this Mac. Scrolling used to be odek's
slowest part and is now in line with the rest.

App size on disk: 19 MB.

## How it's measured

- **Memory:** the physical footprint from `footprint`, sampled every two
  seconds; window buffers from `vmmap` (the window's `CA Whippet Drawable`
  surfaces); odek's own data from `heap` (live allocations). Each scenario runs
  in a fresh odek with its own workspace, three times, interleaved. Settings:
  82% opacity with blur.
- **Throughput:** [vtebench](https://github.com/alacritty/vtebench) (commit
  `ead8003`), which times how fast a terminal reads fixed payloads from the
  pty, in samples of at least 1 MiB; reported as the median ms per sample. Ten
  of its twelve benchmarks run on macOS (`cursor_motion` and `light_cells` find
  the terminal size with `ps -o tty=`, whose output macOS pads with spaces).
  [termbench](https://github.com/cmuratori/termbench) V2 (commit `074fa61`),
  in its small (about 1 MB) and regular (1 GiB of text) sizes.
- **Conditions:** power adapter connected, the Mac kept awake, the same window
  size for every run.

odek's own memory without window buffers can also be measured off screen with
the `mem` step of `scripts/termsnap.sh` (build with `--features selftest`); see
[development.md](development.md).
