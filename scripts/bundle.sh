#!/bin/bash
# Build the release binary, wrap it in a .app, and (with --install) copy it to
# ~/Applications and put a `BIN` command on your PATH.
#
#   scripts/bundle.sh            # → dist/APP.app
#   scripts/bundle.sh --install  # → ~/Applications/APP.app + ~/.local/bin/BIN
set -euo pipefail
cd "$(dirname "$0")/.."

APP="Odek"          # display name
BIN="odek"           # cargo binary name and terminal command
ID="com.hikvineh.$BIN"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

cargo build --release
app="dist/$APP.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/release/$BIN" "$app/Contents/MacOS/$BIN"
[ -f assets/AppIcon.icns ] && cp assets/AppIcon.icns "$app/Contents/Resources/"
cp LICENSE THIRD_PARTY_NOTICES.md "$app/Contents/Resources/"

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
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Folder</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key><array><string>public.folder</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Text</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key><array><string>public.text</string><string>public.source-code</string><string>public.data</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

codesign --force --sign - "$app" >/dev/null
echo "built $app ($(du -sh "$app" | cut -f1))"

if [ "${1:-}" = "--install" ]; then
  mkdir -p ~/Applications ~/.local/bin
  rm -rf ~/Applications/"$APP.app"
  cp -R "$app" ~/Applications/
  cat > ~/.local/bin/"$BIN" <<SH
#!/bin/bash
# Open a folder (default: current directory) or file in $APP.
target=\$(cd "\$(dirname "\${1:-.}")" && pwd)/\$(basename "\${1:-.}")
[ "\${1:-.}" = "." ] && target=\$PWD
exec open -n -a "\$HOME/Applications/$APP.app" --args "\$target"
SH
  chmod +x ~/.local/bin/"$BIN"
  echo "installed ~/Applications/$APP.app and ~/.local/bin/$BIN"
  case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "note: add ~/.local/bin to PATH";; esac
fi
