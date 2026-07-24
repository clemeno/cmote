#!/bin/sh
# Wrap the release binary in a minimal cmote.app so Finder launches it as a GUI
# app (no Terminal window) — see PLAN §11. Run after `cargo build --release`.
set -eu

BIN="target/release/cmote"
APP="target/release/cmote.app"
VERSION="1.1.0"

[ -x "$BIN" ] || { echo "missing $BIN — run: cargo build --release" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/cmote"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>cmote</string>
	<key>CFBundleDisplayName</key><string>cmote</string>
	<key>CFBundleIdentifier</key><string>com.spirtech.cmote</string>
	<key>CFBundleExecutable</key><string>cmote</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "built $APP — launch: open $APP"
