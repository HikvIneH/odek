#!/bin/bash
# Regenerate assets/AppIcon.icns and assets/icon-1024.png from make-icon.swift.
# 16 and 32 px slots get simplified artwork; everything else is the full mark.
set -euo pipefail
cd "$(dirname "$0")"
swift make-icon.swift icon-1024.png 1024 full
set_dir=AppIcon.iconset
rm -rf "$set_dir"
mkdir "$set_dir"
draw() { swift make-icon.swift "$set_dir/$1" "$2" "$3" >/dev/null; }
draw icon_16x16.png 16 small16
draw icon_16x16@2x.png 32 small32
draw icon_32x32.png 32 small32
draw icon_32x32@2x.png 64 full
for s in 128 256 512; do
  draw "icon_${s}x${s}.png" "$s" full
  draw "icon_${s}x${s}@2x.png" $((s * 2)) full
done
iconutil -c icns "$set_dir" -o AppIcon.icns
rm -rf "$set_dir"
echo "wrote assets/AppIcon.icns"
