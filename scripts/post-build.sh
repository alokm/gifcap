#!/usr/bin/env bash
# Post-build: inject Info.plist keys Tauri 2 doesn't support natively,
# then codesign with a stable identifier and copy to ~/Desktop/caps.
set -e

APP="target/release/bundle/macos/GifCap.app"
PLIST="$APP/Contents/Info.plist"
DEST="$HOME/Desktop/caps/GifCap.app"

echo "→ Injecting NSScreenCaptureUsageDescription into Info.plist"
/usr/libexec/PlistBuddy -c \
  "Add :NSScreenCaptureUsageDescription string 'GifCap needs screen recording access to capture GIFs.'" \
  "$PLIST" 2>/dev/null || \
/usr/libexec/PlistBuddy -c \
  "Set :NSScreenCaptureUsageDescription 'GifCap needs screen recording access to capture GIFs.'" \
  "$PLIST"

echo "→ Codesigning with identifier io.gifcap"
codesign --force --deep --sign - --identifier "io.gifcap" "$APP"

echo "→ Copying to $DEST"
rm -rf "$DEST"
cp -R "$APP" "$DEST"

echo "✓ Done — $DEST"
