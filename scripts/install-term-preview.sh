#!/bin/bash
# Build the terminal and install it as a separate preview app, leaving the
# regular Odek.app alone:  ~/Applications/Odek Terminal Preview.app
set -euo pipefail
cd "$(dirname "$0")/.."

APP="Odek Terminal Preview"
BIN="odek-term"
ID="com.hikvineh.odek.term-preview"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
REV=$(/usr/bin/git rev-parse --short HEAD)

cargo build --release
app="dist/$APP.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/odek "$app/Contents/MacOS/$BIN"
[ -f assets/AppIcon.icns ] && cp assets/AppIcon.icns "$app/Contents/Resources/"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP</string>
  <key>CFBundleDisplayName</key><string>$APP</string>
  <key>CFBundleExecutable</key><string>$BIN</string>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION-$REV</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --sign - "$app" >/dev/null
mkdir -p ~/Applications
rm -rf ~/Applications/"$APP.app"
cp -R "$app" ~/Applications/
echo "installed ~/Applications/$APP.app ($REV)"
