#!/bin/bash
# Build the release binary, wrap it in a .app, and (with --install) copy it to
# ~/Applications and put a `BIN` command on your PATH.
#
#   scripts/bundle.sh            # → dist/APP.app
#   scripts/bundle.sh --install  # → ~/Applications/APP.app + ~/.local/bin/BIN
#   scripts/bundle.sh --zip      # → dist/APP-VERSION.zip, the release download
#
# Signs ad hoc unless ODEK_SIGN_IDENTITY names a code-signing certificate in
# your keychain (releases use one so macOS keeps an update's permissions).
set -euo pipefail
cd "$(dirname "$0")/.."

APP="Odek"          # display name
BIN="odek"           # cargo binary name and terminal command
ID="com.hikvineh.$BIN"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

# Keep build-machine paths (home folder, user name) out of the binary.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~"
cargo build --release
app="dist/$APP.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "${CARGO_TARGET_DIR:-target}/release/$BIN" "$app/Contents/MacOS/$BIN"
[ -f assets/AppIcon.icns ] && cp assets/AppIcon.icns "$app/Contents/Resources/"
cp LICENSE THIRD_PARTY_NOTICES.md "$app/Contents/Resources/"
cp "scripts/$BIN" "$app/Contents/Resources/$BIN"

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

codesign --force --sign "${ODEK_SIGN_IDENTITY:--}" "$app" >/dev/null
echo "built $app ($(du -sh "$app" | cut -f1))"

case "${1:-}" in
--install)
  mkdir -p ~/Applications ~/.local/bin
  rm -rf ~/Applications/"$APP.app"
  cp -R "$app" ~/Applications/
  ln -sf ~/Applications/"$APP.app/Contents/Resources/$BIN" ~/.local/bin/"$BIN"
  echo "installed ~/Applications/$APP.app and ~/.local/bin/$BIN"
  case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "note: add ~/.local/bin to PATH";; esac
  ;;
--zip)
  zip="dist/$APP-$VERSION.zip"
  rm -f "$zip"
  ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
  echo "zipped $zip  sha256 $(shasum -a 256 "$zip" | cut -d' ' -f1)"
  ;;
esac
