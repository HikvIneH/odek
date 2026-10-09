#!/usr/bin/env python3
"""Regenerate THIRD_PARTY_NOTICES.md from `cargo metadata`.

Usage: python3 scripts/notices.py > THIRD_PARTY_NOTICES.md
"""
import json
import subprocess

meta = json.loads(
    subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked"], text=True)
)
root = meta["resolve"]["root"]
ids = {n["id"] for n in meta["resolve"]["nodes"]} - {root}
pkgs = sorted((p for p in meta["packages"] if p["id"] in ids), key=lambda p: p["name"].lower())

print("# Third-party notices\n")
print("Odek is MIT licensed. It is built with the following Rust crates, each under")
print("its own license (as declared on crates.io). Their source is available from")
print("crates.io and the repositories listed below; none are modified.\n")
print("| Crate | Version | License | Source |")
print("|---|---|---|---|")
for p in pkgs:
    repo = p.get("repository") or f"https://crates.io/crates/{p['name']}"
    print(f"| {p['name']} | {p['version']} | {p.get('license') or 'see source'} | {repo} |")
print()
print("`nucleo-matcher` is licensed under the Mozilla Public License 2.0; its source")
print("is available at the repository above and on crates.io.")
