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
agent, agent_py, trigger = f"{tmp}/claude", f"{tmp}/agent.py", f"{tmp}/ask"

# A pretend Claude Code session, drawn the way Claude Code (light theme) draws
# itself: header, task bar, an edit shown as a diff, steps that collapse into a
# summary, the spinner and the input box pinned to the bottom. It sets the tab
# title like Claude does, and either keeps working or (given a commit message)
# waits for the trigger file, rings the bell and asks permission to commit.
# The screen is redrawn every tick, so it reflows when the pane is resized.
AGENT_PY = r'''import os, random, re, select, subprocess, sys, termios, time, tty

title, task, step, also, file, run, result, commit = (sys.argv[1:] + [""])[:8]
step = float(step)
E = "\x1b["
rgb = lambda c, bg=False: f"{E}{48 if bg else 38};2;{c}m"
CORAL, GREY, BLUE, GREEN = rgb("215;119;87"), rgb("110;110;110"), rgb("87;105;247"), rgb("44;160;44")
FG, BOLD, OFF = E + "39m", E + "1m", E + "0m"
BAR, MINUS, PLUS = rgb("236;236;236", True), rgb("255;220;224", True), rgb("218;250;218", True)
MASCOT = (" ▐▛███▜▌ ", "▝▜█████▛▘", "  ▘▘ ▝▝  ")
GLYPHS, MOONS = "·✢✳✶✻✽✻✶✳✢", "◐◓◑◒"
TRIGGER = "TRIGGER_PATH"
TIP = "Tip: Use ctrl+v to paste images from your clipboard"
DIFFS = {
    "src/term/vt.rs": (281, ["  let keep = self.prompt_rows();",
                             "- let prompt_row = self.reflow_main(cols, rows, keep);",
                             "+ let prompt_row = self.reflow_main(cols, rows, keep)",
                             "+     .or_else(|| self.live_prompt_row());",
                             "  // Old marks pointed at lines that moved."]),
    "src/highlight.rs": (118, ['  "yaml" | "yml" => Lang::Yaml,',
                               '+ "toml" => Lang::Toml,',
                               "  _ => return None,"]),
    "index.html": (14, ['  <a class="cta" href="#install">',
                        "-   Install",
                        "+   Download for macOS",
                        "  </a>"]),
}
t0, tokens, content, tick = time.time(), 0, [], 0


def plain(s):
    return re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", s)


def fit(s, cols):
    # Cut a styled line to the width, keeping its escape codes.
    out, n = [], 0
    for tok in re.findall(r"\x1b\[[0-9;?]*[A-Za-z]|.", s):
        if tok.startswith("\x1b"):
            out.append(tok)
        elif n < cols:
            out.append(tok)
            n += 1
    return "".join(out)


def diff_block(cols):
    first, lines = DIFFS[file]
    width = min(cols - 8, 64)
    out, n = [], first
    for line in lines:
        mark, text = line[0], line[2:]
        num = f"{n:>5} " if mark != "-" else "      "
        body = f"{mark}{text}"[:width].ljust(width)
        bg = MINUS if mark == "-" else PLUS if mark == "+" else ""
        out.append(f"    {GREY}{num}{OFF}{bg}{body}{OFF}")
        n += mark != "-"
    return out


def draw(spinner=None, tip=False, dialog=False):
    global tick
    tick += 1
    cols, rows = os.get_terminal_size()
    glyph = "✳" if spinner is None else MOONS[tick // 2 % 4]
    head = [f"{BOLD}Claude Code{OFF} {GREY}v2.1.296{OFF}", f"{GREY}Opus 5.5 with medium effort · Claude Max{OFF}",
            f"{GREY}{os.getcwd()}{OFF}"]
    top = ["", *(f" {CORAL}{m}{OFF}  {h}" for m, h in zip(MASCOT, head)), "", ""]
    bar = f"{GREY}❯ {FG}{task}"
    top += [f"{BAR}{bar}{' ' * max(0, cols - len(plain(bar)))}{OFF}", ""]
    for item in content:
        top += diff_block(cols) if item == "DIFF" else [item]
    if dialog:
        dash = f"{GREY}{'╌' * cols}{OFF}"
        top += ["", f"{BLUE}{'─' * cols}{OFF}", f" {BLUE}{BOLD}Bash command{OFF}",
                f" {GREY}Commit the fix{OFF}", dash, f' git commit -m "{commit}"', dash,
                " This command requires approval", "", " Do you want to proceed?",
                f" {BLUE}❯{OFF} {GREY}1.{OFF} {BLUE}Yes{OFF}",
                f"   {GREY}2.{OFF} Yes, and don’t ask again for: git commit *",
                f"   {GREY}3.{OFF} No", "", f" {GREY}Esc to cancel · Tab to amend{OFF}"]
        bottom = []
    else:
        rule = f"{GREY}{'─' * cols}{OFF}"
        bottom = [spinner or "", f"  {GREY}⎿  {TIP}{OFF}" if tip else "", "", rule,
                  f"{GREY}❯{OFF} {E}7m {OFF}", rule, f"  {GREY}? for shortcuts{OFF}"]
    screen = [""] * rows
    for i, line in enumerate(top[:rows]):
        screen[i] = line
    for i, line in enumerate(bottom):
        screen[rows - len(bottom) + i] = line
    sys.stdout.write(f"\x1b]0;{glyph} {title}\x07\x1b[?25l"
                     + "".join(f"{E}{r + 1};1H{fit(l, cols)}{OFF}{E}K" for r, l in enumerate(screen)))
    sys.stdout.flush()


def spin(verb, seconds=None, until=None, tip=False):
    global tokens
    i = 0
    while (seconds is None or i * 0.15 < seconds) and not (until and until()):
        tokens += random.randint(15, 60)
        draw(f"{CORAL}{GLYPHS[i % len(GLYPHS)]} {verb}…{OFF} {GREY}({int(time.time() - t0)}s · ↓ {tokens} tokens){OFF}",
             tip=tip)
        time.sleep(0.15)
        i += 1


def working(doing, detail):
    return [f"{GREY}●{OFF} {doing}", f"  {GREY}⎿  {detail}{OFF}"]


def summary(text):
    return [f"  {GREY}{text}{OFF}", ""]


name = os.path.basename(file)
spin("Pondering", step)
content[:] = working("Reading 2 files…", f"{file}, {also}")
spin("Reading", step)
content[:] = summary("Read 2 files") + [f"{GREEN}●{OFF} {BOLD}Update{OFF}({file})", "DIFF", ""]
spin("Cogitating", step * 1.4, tip=bool(commit))
edited = content[2:]
content[:] = summary("Read 2 files") + edited + working(f"Running {run}…", f"$ {run}")
spin("Noodling", step)
content[:] = edited + summary("Read 2 files, ran 1 shell command") + [f"{FG}●{OFF} {result}"]
if not commit:
    content += [""]
    spin("Pondering")
spin("Pondering", until=lambda: os.path.exists(TRIGGER))
sys.stdout.write("\a")
old = termios.tcgetattr(0)
tty.setcbreak(0)
try:
    while True:
        draw(dialog=True)
        if select.select([0], [], [], 0.15)[0] and os.read(0, 16) in (b"\r", b"\n", b"1"):
            break
finally:
    termios.tcsetattr(0, termios.TCSADRAIN, old)
subprocess.run(["git", "commit", "-q", "--allow-empty", "-m", commit])
head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
content[:] = edited + summary("Read 2 files, ran 2 shell commands") + [
    f"{FG}●{OFF} Committed {BLUE}{head}{OFF}. {result}", "",
    f"{GREY}✻ Crunched for {int(time.time() - t0)}s{OFF}"]
while True:
    draw()
    time.sleep(0.15)
'''.replace("TRIGGER_PATH", trigger)

# Started through a tiny launcher named "claude": odek doesn't count shells as
# running programs, and the pane shows the program's name until a title is set.
AGENT_C = r"""#include <unistd.h>
#include <sys/wait.h>
int main(int argc, char **argv) {
    char *args[argc + 2];
    args[0] = "PYTHON";
    args[1] = "AGENT_PY";
    for (int i = 1; i <= argc; i++) args[i + 1] = argv[i];  /* argv[argc] is NULL */
    pid_t pid = fork();
    if (pid == 0) execv(args[0], args);
    waitpid(pid, 0, 0);
    return 0;
}
""".replace("PYTHON", sys.executable).replace("AGENT_PY", agent_py)


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
# The look of a usual odek: system light/dark, translucent with blur. The agents
# name their own tabs, as Claude Code does.
SETTINGS = {"terminalTheme": "System", "terminalOpacity": "82", "terminalBlur": "1"}
steps += ["appearance light", *(f"setting {k} {v}" for k, v in SETTINGS.items()), "renamegroup odek",
          f"line {agent} 'Reflow fix' 'Fix reflow when the window narrows' 0.9 src/term/grid.rs "
          "src/term/vt.rs 'cargo test -q' 'All 214 tests pass.' 'Keep prompt marks on reflow'",
          "newtab",
          f"line {agent} 'TOML highlighting' 'Highlight TOML in the code viewer' 1.4 Cargo.toml "
          "src/highlight.rs 'cargo build --release' 'The release build finished.'",
          "newtab", "rename zsh",
          "newtab", "group website",
          f"line cd {site} && {agent} 'Download button' 'Add a download button' 1.2 styles.css "
          "index.html 'npm run build' 'The build passes.'",
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
steps += [*(f"setting {k}" for k in SETTINGS), "quit"]

# The projects the agents work in: a clone of this repo and a tiny website.
shots, zdot, cards = f"{tmp}/shots", f"{tmp}/zdot", f"{tmp}/cards"
for d in (shots, zdot, cards):
    os.makedirs(d)
for path, text in ((agent_py, AGENT_PY), (f"{tmp}/claude.c", AGENT_C)):
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

# Your own zsh and prompt (MY_PROMPT=0 for a plain one), but nothing from this
# Claude Code session.
env = dict(
    {k: v for k, v in os.environ.items() if not k.startswith(("CLAUDE", "ANTHROPIC"))},
    GIT_PAGER="cat",
    PAGER="cat",
    **({} if os.environ.get("MY_PROMPT", "1") == "1" else {"ZDOTDIR": zdot}),
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
