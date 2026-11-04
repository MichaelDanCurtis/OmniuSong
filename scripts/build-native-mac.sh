#!/bin/bash
set -euo pipefail

if [[ "$#" -gt 1 ]]; then
  printf '%s\n' 'Usage: build-native-mac.sh [--release|--debug]' >&2
  exit 2
fi
case "${1---release}" in
  --release) OMNIUSONG_PROFILE=release ;;
  --debug) OMNIUSONG_PROFILE=debug ;;
  --help|-h)
    printf '%s\n' 'Usage: build-native-mac.sh [--release|--debug]' 'Default: optimized release build with overflow checks. --debug is for diagnostics.'
    exit 0
    ;;
  *)
    printf '%s\n' 'Usage: build-native-mac.sh [--release|--debug]' >&2
    exit 2
    ;;
esac

if [[ "$(uname -s)" != Darwin ]]; then
  printf '%s\n' 'This development bundle builder requires macOS.' >&2
  exit 1
fi

OMNIUSONG_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$OMNIUSONG_ROOT"
OMNIUSONG_HOST="$(rustc -vV | sed -n 's/^host: //p')"
if [[ -z "$OMNIUSONG_HOST" || "$OMNIUSONG_HOST" != *-apple-darwin ]]; then
  printf '%s\n' 'Cannot determine the native macOS Rust host.' >&2
  exit 1
fi
if [[ -n "${CARGO_BUILD_TARGET:-}" && "$CARGO_BUILD_TARGET" != "$OMNIUSONG_HOST" ]]; then
  printf '%s\n' 'This bundle builder requires the native macOS host target.' >&2
  exit 2
fi
OMNIUSONG_CARGO_ARGS=(build --locked -p omniusong-app --target-dir "$OMNIUSONG_ROOT/target")
if [[ "$OMNIUSONG_PROFILE" == release ]]; then
  OMNIUSONG_CARGO_ARGS+=(--release)
fi
printf 'Building OmniuSong Native (%s profile)\n' "$OMNIUSONG_PROFILE"
mkdir -p .runtime/native-builds
OMNIUSONG_BUILD_LOG="$(mktemp "$OMNIUSONG_ROOT/.runtime/native-builds/cargo-$OMNIUSONG_PROFILE.XXXXXX")"
cargo "${OMNIUSONG_CARGO_ARGS[@]}" --message-format=json-render-diagnostics > "$OMNIUSONG_BUILD_LOG"
OMNIUSONG_ARTIFACT_COUNT=0
OMNIUSONG_OWNED_COUNT=0
OMNIUSONG_OWNED_SEEN='|'
OMNIUSONG_EXECUTABLE=''
OMNIUSONG_OPT_LEVEL=''
while IFS= read -r OMNIUSONG_RECORD; do
  if [[ "$OMNIUSONG_RECORD" == *'"reason":"compiler-artifact"'* && "$OMNIUSONG_RECORD" == *omniusong* ]]; then
    OMNIUSONG_TARGET_NAME="$(printf '%s' "$OMNIUSONG_RECORD" | plutil -extract target.name raw -o - -)"
    OMNIUSONG_TARGET_KIND="$(printf '%s' "$OMNIUSONG_RECORD" | plutil -extract target.kind.0 raw -o - -)"
    OMNIUSONG_TARGET_SOURCE="$(printf '%s' "$OMNIUSONG_RECORD" | plutil -extract target.src_path raw -o - -)"
    OMNIUSONG_IS_TEST="$(printf '%s' "$OMNIUSONG_RECORD" | plutil -extract profile.test raw -o - -)"
    if [[ "$OMNIUSONG_TARGET_SOURCE" == "$OMNIUSONG_ROOT/crates/"* && "$OMNIUSONG_IS_TEST" == false ]]; then
      OMNIUSONG_OWNED_OPT_LEVEL="$(printf '%s' "$OMNIUSONG_RECORD" | plutil -extract profile.opt_level raw -o - -)"
      OMNIUSONG_OVERFLOW="$(printf '%s' "$OMNIUSONG_RECORD" | plutil -extract profile.overflow_checks raw -o - -)"
      OMNIUSONG_FEATURES="$(printf '%s' "$OMNIUSONG_RECORD" | plutil -extract features json -o - -)"
      if [[ "$OMNIUSONG_OVERFLOW" != true || ( "$OMNIUSONG_PROFILE" == release && "$OMNIUSONG_OWNED_OPT_LEVEL" != 3 ) || ( "$OMNIUSONG_PROFILE" == debug && "$OMNIUSONG_OWNED_OPT_LEVEL" != 0 ) || "$OMNIUSONG_FEATURES" == *'"test-hooks"'* ]]; then
        printf '%s\n' 'Cargo artifact does not match the required optimization/overflow-check profile.' >&2
        exit 1
      fi
      case "$OMNIUSONG_OWNED_SEEN" in
        *"|$OMNIUSONG_TARGET_NAME|"*)
          printf '%s\n' 'Cargo reported a duplicate application-owned artifact.' >&2
          exit 1
          ;;
      esac
      OMNIUSONG_OWNED_COUNT=$((OMNIUSONG_OWNED_COUNT + 1))
      OMNIUSONG_OWNED_SEEN="$OMNIUSONG_OWNED_SEEN$OMNIUSONG_TARGET_NAME|"
      printf '%s\n' "$OMNIUSONG_RECORD" > "$OMNIUSONG_BUILD_LOG.owned-$OMNIUSONG_OWNED_COUNT.json"
      if [[ "$OMNIUSONG_TARGET_NAME" == omniusong-app && "$OMNIUSONG_TARGET_KIND" == bin && "$OMNIUSONG_TARGET_SOURCE" == "$OMNIUSONG_ROOT/crates/omniusong-app/src/main.rs" ]]; then
        OMNIUSONG_ARTIFACT_COUNT=$((OMNIUSONG_ARTIFACT_COUNT + 1))
        OMNIUSONG_EXECUTABLE="$(printf '%s' "$OMNIUSONG_RECORD" | plutil -extract executable raw -o - -)"
        OMNIUSONG_OPT_LEVEL="$OMNIUSONG_OWNED_OPT_LEVEL"
        printf '%s\n' "$OMNIUSONG_RECORD" > "$OMNIUSONG_BUILD_LOG.selected.json"
      fi
    fi
  fi
done < "$OMNIUSONG_BUILD_LOG"
for OMNIUSONG_REQUIRED_TARGET in omniusong_core omniusong_recording omniusong_store omniusong_ipc omniusong_engine omniusong_session omniusong-app; do
  case "$OMNIUSONG_OWNED_SEEN" in
    *"|$OMNIUSONG_REQUIRED_TARGET|"*) ;;
    *)
      printf '%s\n' 'Cargo did not report every required application-owned artifact.' >&2
      exit 1
      ;;
  esac
done
if [[ "$OMNIUSONG_ARTIFACT_COUNT" != 1 || ! -x "$OMNIUSONG_EXECUTABLE" ]]; then
  printf '%s\n' 'Cargo did not report exactly one native application executable.' >&2
  exit 1
fi
case "$OMNIUSONG_EXECUTABLE" in
  "$OMNIUSONG_ROOT/target/$OMNIUSONG_PROFILE/omniusong-app"|"$OMNIUSONG_ROOT/target/$OMNIUSONG_HOST/$OMNIUSONG_PROFILE/omniusong-app") ;;
  *)
    printf '%s\n' 'Cargo artifact uses an unsupported target or output location.' >&2
    exit 1
    ;;
esac
lipo -verify_arch "$(uname -m)" "$OMNIUSONG_EXECUTABLE"
mkdir -p dist
OMNIUSONG_STAGE="$(mktemp -d "$OMNIUSONG_ROOT/dist/.native-mac.XXXXXX")"
OMNIUSONG_BUNDLE="$OMNIUSONG_STAGE/OmniuSong Native.app"
mkdir -p "$OMNIUSONG_BUNDLE/Contents/MacOS"
cp "$OMNIUSONG_EXECUTABLE" "$OMNIUSONG_BUNDLE/Contents/MacOS/OmniuSongNative"
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
plutil -insert OmniuSongBuildProfile -string "$OMNIUSONG_PROFILE" "$OMNIUSONG_BUNDLE/Contents/Info.plist"
plutil -insert OmniuSongBuildOptLevel -string "$OMNIUSONG_OPT_LEVEL" "$OMNIUSONG_BUNDLE/Contents/Info.plist"
plutil -insert OmniuSongBuildOverflowChecks -bool YES "$OMNIUSONG_BUNDLE/Contents/Info.plist"
plutil -lint "$OMNIUSONG_BUNDLE/Contents/Info.plist"
codesign --force --sign - --identifier local.omniusong.native-preview "$OMNIUSONG_BUNDLE"
if [[ -e "$OMNIUSONG_ROOT/dist/OmniuSong Native.app" ]]; then
  mv "$OMNIUSONG_ROOT/dist/OmniuSong Native.app" "$OMNIUSONG_ROOT/dist/OmniuSong Native.previous-$(date +%Y%m%dT%H%M%S).app"
fi
mv "$OMNIUSONG_BUNDLE" "$OMNIUSONG_ROOT/dist/OmniuSong Native.app"
rmdir "$OMNIUSONG_STAGE"
printf 'Cargo artifact receipt: %s\n' "$OMNIUSONG_BUILD_LOG.selected.json"
printf '%s\n' "$OMNIUSONG_ROOT/dist/OmniuSong Native.app"
