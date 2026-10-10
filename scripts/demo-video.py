#!/usr/bin/env python3
"""Record the launch video (odek-demo.mp4) off-screen.

    cargo build --release --features selftest
    scripts/demo-video.py [out.mp4]
    CAPTIONS=1 scripts/demo-video.py [out.mp4]   # with captions and title cards

Like demo-gif.py, but tells the coding-agent story: agents run in tabs grouped
by project, one stops to ask a question and its dot turns orange, you jump to
it, read the line it changed beside the terminal and let it carry on. The
agents are a small shell script that prints what an agent would, so the run is
the same every time. Needs git and ffmpeg, plus Pillow for captions.
"""
import hashlib, os, re, shutil, subprocess, sys, tempfile, time

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out_mp4 = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else f"{root}/target/odek-demo.mp4")
captions = bool(os.environ.get("CAPTIONS"))
live = bool(os.environ.get("LIVE"))  # record the real window on screen (needs Screen Recording)
base = "/tmp/odek-demo"
demo, site = f"{base}/odek", f"{base}/website"
tmp = tempfile.mkdtemp(prefix="odek-video-")
agent, agent_sh, trigger = f"{tmp}/claude", f"{tmp}/agent.sh", f"{tmp}/ask"

# A pretend Claude Code session: reads, edits and runs something with a spinner
# in between, then keeps working, or (given a commit message) waits for the
# trigger file, rings the bell and asks permission to commit.
AGENT_SH = r"""# agent.sh <task> <step seconds> <also read> <file> <adds> <removes> <run> <result> [commit message]
task=$1 step=$2 also=$3 file=$4 adds=$5 dels=$6 run=$7 result=$8 commit=$9
coral=$'\e[38;2;215;119;87m' green=$'\e[38;2;78;186;101m' purple=$'\e[38;2;177;185;249m'
grey=$'\e[38;2;153;153;153m' bold=$'\e[1m' off=$'\e[0m'
glyphs=(· ✢ ✳ ✶ ✻ ✽ ✻ ✶ ✳ ✢)
n=$(awk "BEGIN { print int($step / 0.15) }")
t0=$SECONDS
spin() {  # spin <frames | wait> <verb>
  local i=0
  while :; do
    if [ "$1" = wait ]; then [ -e TRIGGER ] && break; elif [ $i -ge "$1" ]; then break; fi
    printf '\r\e[K%s%s %s…%s %s(%ds · esc to interrupt)%s' "$coral" "${glyphs[i % 10]}" "$2" "$off" \
      "$grey" $((SECONDS - t0)) "$off"
    sleep 0.15; i=$((i + 1))
  done
  printf '\r\e[K'
}
tool() { printf '%s⏺%s %s%s%s(%s)\n  %s⎿  %s%s\n\n' "$green" "$off" "$bold" "$1" "$off" "$2" "$grey" "$3" "$off"; }
rule() { printf '%s%s%s\n' "$1" "$(printf '─%.0s' $(seq 1 48))" "$off"; }

printf '\e[?25l\e[H\e[2J\n%s> %s%s\n\n' "$grey" "$task" "$off"
spin "$n" Pondering
tool Read "$file" "Read $(wc -l < "$file" | tr -d ' ') lines"
tool Read "$also" "Read $(wc -l < "$also" | tr -d ' ') lines"
spin "$n" Cogitating
tool Update "$file" "Updated with $adds additions and $dels removals"
spin "$n" Noodling
tool Bash "$run" "$result"
if [ -z "$commit" ]; then spin 99999 Pondering; exit; fi
spin wait Clauding
printf '\a'
rule "$purple"
printf ' %sBash command%s\n\n   git commit -m "%s"\n\n' "$bold$purple" "$off" "$commit"
printf ' Do you want to proceed?\n %s❯ 1. Yes%s\n   2. No, and tell Claude what to do\n      differently (esc)\n' "$purple" "$off"
read -rsn1 answer
printf '\e[9A\e[J'
git commit -q --allow-empty -m "$commit"
tool Bash "git commit -m \"$commit\"" "[main $(git rev-parse --short HEAD)] $commit"
printf '%s⏺%s Committed. The prompt now stays put when\n  the window narrows.\n\n' "$off" "$off"
rule "$grey"
printf '%s>%s\n' "$grey" "$off"
rule "$grey"
sleep 600
""".replace("TRIGGER", trigger)

# Started through a tiny launcher named "claude": odek doesn't count shells as
# running programs, and the pane title shows the program's name.
AGENT_C = r"""#include <unistd.h>
#include <sys/wait.h>
int main(int argc, char **argv) {
    char *args[argc + 2];
    args[0] = "/bin/bash";
    args[1] = "AGENT_SH";
    for (int i = 1; i <= argc; i++) args[i + 1] = argv[i];  /* argv[argc] is NULL */
    pid_t pid = fork();
    if (pid == 0) execv(args[0], args);
    waitpid(pid, 0, 0);
    return 0;
}
""".replace("AGENT_SH", agent_sh)

steps, frames = [], []  # frames: (name, seconds on screen; 0 = only if it changed, caption)
caption = ""


def snap(secs):
    if live:  # the screen recording sees it; just let it stay on screen
        steps.append(f"wait {secs}")
        return
    name = f"f{len(frames):03d}"
    steps.append(f"snapws {name}")
    frames.append((name, secs, caption))


def hold(seconds, every=0.5):
    # Keep snapping while things happen on their own (agents printing).
    if live:
        steps.append(f"wait {seconds}")
        return
    for _ in range(int(seconds / every)):
        steps.append(f"wait {every}")
        snap(every)


def run(cmd, hold_secs=2.0):
    # Type a command into the shell a key at a time, then run it.
    for chunk in re.findall(r" *[^ ]", cmd):  # step lines are trimmed
        steps.extend([f"keys {chunk}", "wait 0.07" if live else "wait 0.03"])
        if not live:
            snap(0.055)
    steps.extend(["keys \\r", "wait 0.6"])
    snap(hold_secs)


# Setup, before the first frame: two projects, four tabs.
steps += ["appearance dark", "setting terminalTheme Dark", "renamegroup odek", "rename reflow fix",
          f"line {agent} 'Fix reflow when the window narrows' 0.9 src/term/grid.rs src/term/vt.rs "
          "12 3 'cargo test -q' 'test result: ok. 214 passed' 'Keep prompt marks on reflow'",
          "newtab", "rename toml highlighting",
          f"line {agent} 'Highlight TOML in the code viewer' 1.4 Cargo.toml src/highlight.rs "
          "9 0 'cargo build --release' 'Finished release in 38.2s'",
          "newtab", "rename zsh",
          "newtab", "group website", "rename landing page",
          f"line cd {site} && {agent} 'Add a download button' 1.2 styles.css index.html "
          "6 1 'npm run build' 'built in 1.4s'",
          "newtab", "rename dev server",
          f"line cd {site} && python3 -m http.server 8080",
          "wait 0.8", "nexttab", "wait 0.3"]

caption = "Run each coding agent in its own tab, grouped by project"
hold(4.0)
steps += ["nexttab", "wait 0.3"]
hold(3.0)
steps += ["nexttab", "wait 0.3"]  # a plain zsh beside the agents
snap(0.5)
run("ls src/term", hold_secs=1.6)
steps += ["nexttab", "wait 0.3"]
hold(2.5)

caption = "A dot turns orange when an agent needs you"
ringed = caption
ask_at = len(frames)  # the question is released as this frame is reached
ask_step = len(steps)  # or, live, when the steps get this far
steps.append("wait 0.8")
hold(3.5)

caption = "Jump straight to it"
steps += ["nexttab", "wait 0.2", "nexttab", "wait 0.4"]
snap(2.2)

caption = "Open the line it changed, right beside the terminal"
steps += [f"open {demo}/src/term/vt.rs:282", "wait 1.2"]
snap(2.6)

caption = "Browse the project tree"
steps += ["files", "wait 1.0"]
snap(2.6)

caption = "Then let it carry on"
steps += ["focusterm", "wait 0.3", "keys \\r", "wait 0.8"]
snap(2.8)

caption = "And check its commit from your shell"
steps += ["nexttab", "wait 0.2", "nexttab", "wait 0.4"]
snap(0.6)
run("git log --oneline -3", hold_secs=3.4)
steps += ["setting terminalTheme", "quit"]

# The projects the agents work in: a clone of this repo and a tiny website.
shots, zdot, cards = f"{tmp}/shots", f"{tmp}/zdot", f"{tmp}/cards"
for d in (shots, zdot, cards):
    os.makedirs(d)
for path, text in ((agent_sh, AGENT_SH), (f"{tmp}/claude.c", AGENT_C)):
    with open(path, "w") as f:
        f.write(text)
subprocess.run(["cc", "-O", "-o", agent, f"{tmp}/claude.c"], check=True)
with open(f"{zdot}/.zshrc", "w") as f:
    f.write("PROMPT='%F{cyan}%1~%f %F{green}❯%f '\nexport GIT_PAGER=cat PAGER=cat\n")
shutil.rmtree(base, ignore_errors=True)
# A plain main at the latest origin/main: no remotes or other branches in the log.
head = subprocess.run(["git", "-C", root, "rev-parse", "origin/main"], capture_output=True, text=True,
                      check=True).stdout.strip()
subprocess.run(["git", "clone", "-q", "--no-checkout", root, demo], check=True)
for args in (["checkout", "-q", "-B", "main", head], ["remote", "remove", "origin"]):
    subprocess.run(["git", "-C", demo, *args], check=True)
for branch in subprocess.run(["git", "-C", demo, "branch", "--format=%(refname:short)"],
                             capture_output=True, text=True).stdout.split():
    if branch != "main":
        subprocess.run(["git", "-C", demo, "branch", "-q", "-D", branch], check=True)
os.makedirs(site)
for name, lines in (("index.html", 86), ("styles.css", 142)):
    with open(f"{site}/{name}", "w") as f:
        f.write("\n" * lines)

env = dict(
    os.environ,
    ZDOTDIR=zdot,
    ODEK_TERM_WS="1",
    ODEK_WORKSPACE_FILE=f"{tmp}/ws.txt",
    ODEK_TERM_SNAP=shots,
    ODEK_TERM_STEPS="\n".join(steps),
    **({"ODEK_TERM_LIVE": "1"} if live else {}),
)
def waits(lines):
    return sum(float(s.split()[1]) for s in lines if s.startswith("wait "))


WINDOW_BOUNDS = r"""
import CoreGraphics
let pid = Int(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
for w in list where (w[kCGWindowOwnerPID as String] as? Int) == pid && (w[kCGWindowLayer as String] as? Int) == 0 {
    let b = w[kCGWindowBounds as String] as! [String: Any]
    print("\(b["X"]!),\(b["Y"]!),\(b["Width"]!),\(b["Height"]!)")
    break
}
"""

if live:
    # Real time on a real window: the screen recording is the video.
    setup = waits(steps[:steps.index("wait 0.8") + 3])
    total = waits(steps)
    with open(f"{tmp}/bounds.swift", "w") as f:
        f.write(WINDOW_BOUNDS)
    start = time.time()
    odek = subprocess.Popen([f"{root}/target/release/odek", "--term", demo], env=env, stdout=subprocess.DEVNULL)
    rect = ""
    while not rect and time.time() - start < 10:
        time.sleep(0.2)
        rect = subprocess.run(["swift", f"{tmp}/bounds.swift", str(odek.pid)], capture_output=True,
                              text=True).stdout.strip()
    if not rect:
        sys.exit("odek's window never appeared")
    mov = f"{tmp}/screen.mov"
    rec = subprocess.Popen(["screencapture", "-v", "-x", "-R", rect, "-V", str(int(total + 4)), mov])
    began = time.time() - start
    time.sleep(max(0, start + waits(steps[:ask_step]) + 1.0 - time.time()))
    open(trigger, "w").close()
    odek.wait()
    rec.wait()
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-ss", f"{max(0, setup + 1.0 - began):.2f}", "-i", mov,
         "-t", f"{total - setup:.2f}", "-vf", "scale=1600:-2:flags=lanczos,fps=30,format=yuv420p",
         "-c:v", "libx264", "-crf", "18", "-movflags", "+faststart", out_mp4],
        check=True,
    )
    print({"mp4": out_mp4, "window": rect, "seconds": round(total - setup, 1)})
    if not os.environ.get("KEEP"):
        shutil.rmtree(tmp)
        shutil.rmtree(base, ignore_errors=True)
    sys.exit()

odek = subprocess.Popen([f"{root}/target/release/odek", "--term", demo], env=env, stdout=subprocess.DEVNULL)
# Snapshots take real time, so release the question by frame, not by clock.
while odek.poll() is None and not os.path.exists(f"{shots}/f{ask_at - 1:03d}.png"):
    time.sleep(0.05)
open(trigger, "w").close()
odek.wait()

WIDTH = 1600
clips = []  # (png, seconds)

if captions:
    from PIL import Image, ImageDraw, ImageFont

    BG, FG, ACCENT, MUTED = (11, 15, 20), (240, 244, 248), (59, 130, 246), (139, 148, 158)
    sf, mono = "/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/Menlo.ttc"

    def font(size, path=sf, weight=None):
        f = ImageFont.truetype(path, size)
        if weight:
            f.set_variation_by_name(weight)
        return f

    first = Image.open(f"{shots}/{frames[0][0]}.png")
    shot_h = round(first.height * WIDTH / first.width / 2) * 2  # H.264 wants even sizes
    band = 150
    size = (WIDTH, shot_h + band)
    cap_font = font(46, weight="Semibold")

    def centred(draw, y, text, f, fill):
        draw.text(((WIDTH - draw.textlength(text, font=f)) / 2, y), text, font=f, fill=fill)

    def card(name, lines):
        # lines: (text, font, colour, y)
        img = Image.new("RGB", size, BG)
        d = ImageDraw.Draw(img)
        for text, f, fill, y in lines:
            centred(d, y, text, f, fill)
        img.save(f"{cards}/{name}.png")
        return f"{cards}/{name}.png"

    def framed(name, text):
        shot = Image.open(f"{shots}/{name}.png").convert("RGB").resize((WIDTH, shot_h), Image.LANCZOS)
        img = Image.new("RGB", size, BG)
        img.paste(shot, (0, 0))
        d = ImageDraw.Draw(img)
        centred(d, shot_h + (band - 56) / 2, text, cap_font, FG)
        if text == ringed:
            # Ring the first tab's dot so it reads at phone size.
            x, y, r = 27 * WIDTH / 1600, 115 * WIDTH / 1600, 13
            d.ellipse((x - r, y - r, x + r, y + r), outline=(255, 159, 10), width=3)
        img.save(f"{cards}/{name}.png")
        return f"{cards}/{name}.png"

    mid = size[1] / 2
    title = [("Another day, another terminal emulator.", font(64, weight="Bold"), FG, mid - 70)]
    clips += [(card("intro1", title), 1.8),
              (card("intro2", title + [("This one is for running coding agents side by side.",
                                        font(44, weight="Medium"), ACCENT, mid + 20)]), 2.6)]
else:
    def framed(name, text):
        return f"{shots}/{name}.png"

# Merge repeated frames; a frame left at 0 s that didn't change adds nothing.
start, last = len(clips), None
for name, secs, text in frames:
    digest = hashlib.md5(open(f"{shots}/{name}.png", "rb").read() + text.encode() * captions).hexdigest()
    secs = secs or 0.5
    if digest == last and len(clips) > start:
        clips[-1] = (clips[-1][0], clips[-1][1] + secs)
    else:
        clips.append((framed(name, text), secs))
    last = digest

if captions:
    lockup = Image.open(f"{root}/assets/odek-lockup.png").convert("RGBA")
    lockup = lockup.resize((520, round(lockup.height * 520 / lockup.width)), Image.LANCZOS)
    end = Image.open(card("end", [
        ("Native Rust + AppKit  ·  about 38 MB idle  ·  free and open source", font(40, weight="Medium"), FG, mid + 10),
        ("brew install --cask hikvineh/tap/odek", font(38, mono), ACCENT, mid + 90),
        ("github.com/HikvIneH/odek", font(38, weight="Medium"), MUTED, mid + 160),
    ])).convert("RGBA")
    end.alpha_composite(lockup, ((WIDTH - lockup.width) // 2, round(mid - 60 - lockup.height)))
    end.convert("RGB").save(f"{cards}/end.png")
    clips.append((f"{cards}/end.png", 4.5))

with open(f"{tmp}/concat.txt", "w") as f:
    for path, secs in clips:
        f.write(f"file '{path}'\nduration {secs:.2f}\n")
    f.write(f"file '{clips[-1][0]}'\n")
subprocess.run(
    ["ffmpeg", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", f"{tmp}/concat.txt",
     "-vf", f"scale={WIDTH}:-2:flags=lanczos,fps=30,format=yuv420p",
     "-c:v", "libx264", "-crf", "18", "-movflags", "+faststart", out_mp4],
    check=True,
)
if os.environ.get("KEEP"):
    print("frames kept in", tmp)
else:
    shutil.rmtree(tmp)
    shutil.rmtree(base, ignore_errors=True)
print({"mp4": out_mp4, "clips": len(clips), "seconds": round(sum(s for _, s in clips), 1)})
