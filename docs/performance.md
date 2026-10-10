# Performance

Measured on an M1 Mac with 8 GB, macOS 26, a zsh with powerlevel10k. Memory is
the process footprint, the number Activity Monitor shows.

**What Activity Monitor shows** includes the window's drawing buffers, which
macOS keeps for any app with a window on screen and which grow with the window.
For one idle shell in a window maximized on a 2560×1600 display it's about
45 MB, 22 MB of it window buffers; while a lot of output is being drawn it can
rise to around 100–120 MB for a moment and then falls back.

odek draws its windows in sRGB. Left to itself, macOS gives windows on a
wide-colour display half-float buffers, twice the size (about 43 MB each for a
maximized window), and keeps two or three of them while anything animates. That
alone took the same window to around 120 MB idle and 200–250 MB under load.

**odek's own memory**, without window buffers (measured with the window off
screen, so it doesn't depend on window size):

| | |
|---|---|
| One shell, idle | ~38 MB |
| Two tabs, three panes | ~39 MB |
| A file open in the code viewer beside a terminal | ~52 MB |
| Six panes, each with a full scrollback | ~70 MB (91 MB peak while all six printed at once) |
| Full scrollback (10,000 lines) | ~2 MB per pane |
| `seq 1 3000000` | 2.9 s, then back to ~30 MB |
| Reflowing 20,000 lines on resize | ~4 ms |
| App size on disk | 19 MB |

Programs you run (a shell, Claude Code, `vim`) are processes of their own and
use the same memory in any terminal; they aren't counted above.

## Reproducing

The off-screen numbers come from the `mem` step of `scripts/termsnap.sh`
(build with `--features selftest`); see [development.md](development.md).
