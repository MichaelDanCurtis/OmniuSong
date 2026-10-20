#!/bin/bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  printf '%s\n' 'This development bundle builder requires macOS.' >&2
  exit 1
fi

OMNIUSONG_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$OMNIUSONG_ROOT"
cargo build --locked -p omniusong-app
mkdir -p dist
OMNIUSONG_STAGE="$(mktemp -d "$OMNIUSONG_ROOT/dist/.native-mac.XXXXXX")"
OMNIUSONG_BUNDLE="$OMNIUSONG_STAGE/OmniuSong Native.app"
mkdir -p "$OMNIUSONG_BUNDLE/Contents/MacOS"
cp target/debug/omniusong-app "$OMNIUSONG_BUNDLE/Contents/MacOS/OmniuSongNative"
chmod 755 "$OMNIUSONG_BUNDLE/Contents/MacOS/OmniuSongNative"
cat > "$OMNIUSONG_BUNDLE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>OmniuSong Native</string>
  <key>CFBundleDisplayName</key><string>OmniuSong Native</string>
  <key>CFBundleIdentifier</key><string>local.omniusong.native-preview</string>
  <key>CFBundleExecutable</key><string>OmniuSongNative</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.2.0</string>
  <key>CFBundleVersion</key><string>0.2.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Record original performances in your local OmniuSong project.</string>
</dict></plist>
PLIST
plutil -lint "$OMNIUSONG_BUNDLE/Contents/Info.plist"
codesign --force --sign - --identifier local.omniusong.native-preview "$OMNIUSONG_BUNDLE"
if [[ -e "$OMNIUSONG_ROOT/dist/OmniuSong Native.app" ]]; then
  mv "$OMNIUSONG_ROOT/dist/OmniuSong Native.app" "$OMNIUSONG_ROOT/dist/OmniuSong Native.previous-$(date +%Y%m%dT%H%M%S).app"
fi
mv "$OMNIUSONG_BUNDLE" "$OMNIUSONG_ROOT/dist/OmniuSong Native.app"
rmdir "$OMNIUSONG_STAGE"
printf '%s\n' "$OMNIUSONG_ROOT/dist/OmniuSong Native.app"
