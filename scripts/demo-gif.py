#!/usr/bin/env python3
"""Record the README animation (docs/odek-demo.gif) off-screen.

    cargo build --release --features selftest
    scripts/demo-gif.py [out.gif]

Clones this repo into /tmp/odek-demo/odek so paths and the prompt look neutral,
drives the workspace window through a short story (type a command, open a file
in the code viewer beside it, run the tests in a new tab), snapshots every step
and encodes the frames with ffmpeg. Needs git and ffmpeg.
"""
import hashlib, json, os, re, shutil, subprocess, sys, tempfile

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out_gif = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else f"{root}/docs/odek-demo.gif")
demo = "/tmp/odek-demo/odek"

steps, frames = [], []  # frames: (name, seconds on screen; 0 = only if it changed)


def snap(secs):
    name = f"f{len(frames):03d}"
    steps.append(f"snapws {name}")
    frames.append((name, secs))


def type_text(text):
    # Step lines are trimmed, so a space rides along with the next character.
    for chunk in re.findall(r" *[^ ]", text):
        steps.extend([f"keys {chunk}", "wait 0.03"])
        snap(0.055)


def run(cmd, settle=0.8, hold=1.8):
    type_text(cmd)
    steps.extend(["keys \\r", f"wait {settle}"])
    snap(hold)


steps += ["appearance dark", "setting terminalTheme Dark", "rename odek", "renamegroup Odek", "wait 1.5"]
snap(1.0)
run("git log --oneline -9")
steps += [f"open {demo}/src/term/vt.rs:286", "wait 1.2"]
snap(2.6)
steps += ["focusterm", "newtab", "rename tests", "wait 1.0"]
snap(0.8)
run("cargo test -q --release 2>&1 | tail -5", settle=1.0, hold=0.9)
steps += ["nexttab", "wait 0.5"]  # back to the first tab while the tests run
snap(1.2)
run("ls src/term", hold=1.4)
for _ in range(30):  # the tests finishing in the background
    steps.append("wait 2")
    snap(0)
steps += ["nexttab", "wait 0.6"]
snap(3.0)
steps += ["setting terminalTheme", "quit"]

tmp = tempfile.mkdtemp(prefix="odek-demo-")
shots = f"{tmp}/shots"
zdot = f"{tmp}/zdot"
os.makedirs(shots)
os.makedirs(zdot)
with open(f"{zdot}/.zshrc", "w") as f:
    f.write(
        "PROMPT='%F{cyan}%1~%f %F{green}❯%f '\n"
        f"export CARGO_TARGET_DIR={root}/target CARGO_TERM_COLOR=always GIT_PAGER=cat PAGER=cat\n"
    )

shutil.rmtree(os.path.dirname(demo), ignore_errors=True)
subprocess.run(["git", "clone", "-q", root, demo], check=True)
env = dict(
    os.environ,
    ZDOTDIR=zdot,
    ODEK_TERM_WS="1",
    ODEK_WORKSPACE_FILE=f"{tmp}/ws.txt",
    ODEK_TERM_SNAP=shots,
    ODEK_TERM_STEPS="\n".join(steps),
)
subprocess.run([f"{root}/target/release/odek", "--term", demo], env=env, stdout=subprocess.DEVNULL)

# Merge repeated frames; a frame left at 0 s that didn't change adds nothing.
merged, last = [], None
for name, secs in frames:
    digest = hashlib.md5(open(f"{shots}/{name}.png", "rb").read()).hexdigest()
    secs = secs or 0.5
    if digest == last:
        merged[-1][1] += secs if secs > 0.5 else 0
    else:
        merged.append([name, secs])
    last = digest
merged[-1][1] = max(merged[-1][1], 3.0)
with open(f"{tmp}/concat.txt", "w") as f:
    for name, secs in merged:
        f.write(f"file '{shots}/{name}.png'\nduration {secs:.2f}\n")
    f.write(f"file '{shots}/{merged[-1][0]}.png'\n")

subprocess.run(
    ["ffmpeg", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", f"{tmp}/concat.txt",
     "-vf", "scale=1200:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=full[p];"
            "[b][p]paletteuse=dither=none",
     "-loop", "0", out_gif],
    check=True,
)
shutil.rmtree(tmp)
shutil.rmtree(os.path.dirname(demo), ignore_errors=True)
print(json.dumps({"gif": out_gif, "frames": len(merged), "seconds": round(sum(s for _, s in merged), 1)}))
