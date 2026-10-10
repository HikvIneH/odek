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
import hashlib, os, shutil, subprocess, sys, tempfile, time

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out_mp4 = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else f"{root}/target/odek-demo.mp4")
captions = bool(os.environ.get("CAPTIONS"))
base = "/tmp/odek-demo"
demo, site = f"{base}/odek", f"{base}/website"
tmp = tempfile.mkdtemp(prefix="odek-video-")
agent, agent_sh, trigger = f"{tmp}/agent", f"{tmp}/agent.sh", f"{tmp}/ask"

# A pretend coding agent: works through a few steps and keeps going, or (with
# a question) waits for the trigger file, rings the bell and asks.
AGENT_SH = r"""# agent.sh <task> <step seconds> <also read> <file:line> <change> <run> <result> [question]
task=$1 step=$2 also=$3 edit=$4 change=$5 run=$6 result=$7 question=$8
dim=$'\e[2m' cyan=$'\e[36m' green=$'\e[32m' yellow=$'\e[33m' bold=$'\e[1m' off=$'\e[0m'
printf '\e[H\e[2J'
printf '\n %s◆ Task%s  %s\n\n' "$bold" "$off" "$task"
say() { sleep "$step"; printf ' %s●%s %s\n' "$cyan" "$off" "$1"; }
say "Read ${edit%%:*}"
say "Read $also"
sleep "$step"; printf ' %s✻ Thinking…%s\n' "$dim" "$off"
say "Edit $edit  $dim$change$off"
say "Run  $run"
sleep "$step"; printf '   %s✓ %s%s\n' "$green" "$result" "$off"
if [ -z "$question" ]; then
  sleep "$step"; printf ' %s✻ Thinking…%s\n' "$dim" "$off"
  sleep 600; exit
fi
until [ -e TRIGGER ]; do sleep 0.1; done
printf '\n %s?%s %s %s(y/n)%s ' "$yellow" "$off" "$question" "$dim" "$off"
printf '\a'
read -r answer
printf '\n %s●%s Commit 3f2a1c4  %sKeep prompt marks on reflow%s\n' "$cyan" "$off" "$dim" "$off"
printf '   %s✓ Done%s\n\n' "$green" "$off"
""".replace("TRIGGER", trigger)

# Started through Python so the tab has a program running: odek doesn't count
# shells as running programs, and a real agent wouldn't be one.
AGENT = f"#!{sys.executable}\nimport subprocess, sys\nsubprocess.run(['/bin/bash', '{agent_sh}', *sys.argv[1:]])\n"

steps, frames = [], []  # frames: (name, seconds on screen; 0 = only if it changed, caption)
caption = ""


def snap(secs):
    name = f"f{len(frames):03d}"
    steps.append(f"snapws {name}")
    frames.append((name, secs, caption))


def hold(seconds, every=0.5):
    # Keep snapping while things happen on their own (agents printing).
    for _ in range(int(seconds / every)):
        steps.append(f"wait {every}")
        snap(every)


# Setup, before the first frame: two projects, four tabs.
steps += ["appearance dark", "setting terminalTheme Dark", "renamegroup odek", "rename reflow fix",
          f"line {agent} 'Fix reflow when the window narrows' 0.9 src/term/grid.rs src/term/vt.rs:282 "
          "'+12 −3' 'cargo test -q' '214 passed' 'Tests pass. Commit the fix?'",
          "newtab", "rename toml highlighting",
          f"line {agent} 'Highlight TOML in the code viewer' 1.4 Cargo.toml src/highlight.rs:95 "
          "'+9 −0' 'cargo build --release' 'built in 38s'",
          "newtab", "group website", "rename landing page",
          f"line cd {site} && {agent} 'Add the download button' 1.2 styles.css index.html:14 "
          "'+6 −1' 'npm run build' 'built in 1.4s'",
          "newtab", "rename dev server",
          f"line cd {site} && python3 -m http.server 8080",
          "wait 0.8", "nexttab", "wait 0.3"]

caption = "Run each coding agent in its own tab, grouped by project"
hold(4.0)
steps += ["nexttab", "wait 0.3"]
hold(3.0)
steps += ["nexttab", "wait 0.3"]
hold(2.5)

caption = "A dot turns orange when an agent needs you"
ringed = caption
ask_at = len(frames)  # the question is released as this frame is reached
steps.append("wait 0.8")
hold(3.5)

caption = "Jump straight to it"
steps += ["nexttab", "wait 0.2", "nexttab", "wait 0.4"]
snap(2.2)

caption = "Open the line it changed, right beside the terminal"
steps += [f"open {demo}/src/term/vt.rs:282", "wait 1.2"]
snap(3.2)

caption = "Then let it carry on"
steps += ["focusterm", "wait 0.3", "keys y", "wait 0.3"]
snap(0.6)
steps += ["keys \\r", "wait 0.8"]
snap(3.0)
steps += ["setting terminalTheme", "quit"]

# The projects the agents work in: a clone of this repo and a tiny website.
shots, zdot, cards = f"{tmp}/shots", f"{tmp}/zdot", f"{tmp}/cards"
for d in (shots, zdot, cards):
    os.makedirs(d)
for path, text in ((agent_sh, AGENT_SH), (agent, AGENT)):
    with open(path, "w") as f:
        f.write(text)
os.chmod(agent, 0o755)
with open(f"{zdot}/.zshrc", "w") as f:
    f.write("PROMPT='%F{cyan}%1~%f %F{green}❯%f '\nexport GIT_PAGER=cat PAGER=cat\n")
shutil.rmtree(base, ignore_errors=True)
subprocess.run(["git", "clone", "-q", root, demo], check=True)
subprocess.run(["git", "-C", demo, "checkout", "-q", "-B", "main"], check=True)
os.makedirs(site)
for name in ("index.html", "styles.css"):
    open(f"{site}/{name}", "w").close()

env = dict(
    os.environ,
    ZDOTDIR=zdot,
    ODEK_TERM_WS="1",
    ODEK_WORKSPACE_FILE=f"{tmp}/ws.txt",
    ODEK_TERM_SNAP=shots,
    ODEK_TERM_STEPS="\n".join(steps),
)
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
