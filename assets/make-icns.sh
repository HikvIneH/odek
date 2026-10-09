#!/bin/bash
# Build assets/AppIcon.icns from assets/icon-1024.png, every size scaled from it.
# With --redraw, first regenerate icon-1024.png from make-icon.swift (which
# draws at the screen's scale, so the result is brought back to 1024 px).
set -euo pipefail
cd "$(dirname "$0")"
if [ "${1:-}" = "--redraw" ]; then
  swift make-icon.swift
  sips -z 1024 1024 icon-1024.png >/dev/null
fi
set_dir=AppIcon.iconset
rm -rf "$set_dir"
mkdir "$set_dir"
size() { sips -z "$2" "$2" icon-1024.png --out "$set_dir/$1" >/dev/null; }
for s in 16 32 128 256 512; do
  size "icon_${s}x${s}.png" "$s"
  size "icon_${s}x${s}@2x.png" $((s * 2))
done
iconutil -c icns "$set_dir" -o AppIcon.icns
rm -rf "$set_dir"
echo "wrote assets/AppIcon.icns"
