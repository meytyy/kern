#!/bin/sh
# Reproducible local KERN.app for macOS arm64. Needs only the Xcode Command Line Tools.
#   scripts/bundle.sh                     release build into dist/KERN.app
#   KERN_FEATURES=autotest scripts/bundle.sh   same, with the scripted test driver
set -eu
cd "$(dirname "$0")/.."

[ "$(uname -m)" = "arm64" ] || { echo "This script targets arm64 Macs" >&2; exit 1; }
xcode-select -p >/dev/null 2>&1 || { echo "Install Command Line Tools: xcode-select --install" >&2; exit 1; }

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
FEATURES=${KERN_FEATURES:-}
if [ -n "$FEATURES" ]; then
    cargo build --release --locked --features "$FEATURES"
else
    cargo build --release --locked
fi

APP=dist/KERN.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/kern "$APP/Contents/MacOS/KERN"

# Icon set derived from the original logo. logo.png itself is never modified.
[ assets/icon-1024.png -nt logo.png ] || swift scripts/make_icon.swift logo.png assets/icon-1024.png
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
SET="$WORK/KERN.iconset"
mkdir "$SET"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" assets/icon-1024.png --out "$SET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" assets/icon-1024.png --out "$SET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o "$APP/Contents/Resources/KERN.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>KERN</string>
    <key>CFBundleDisplayName</key><string>KERN</string>
    <key>CFBundleIdentifier</key><string>dev.kern.ide</string>
    <key>CFBundleExecutable</key><string>KERN</string>
    <key>CFBundleIconFile</key><string>KERN</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature: enough for this Mac. Not for distribution.
codesign --force --deep --sign - "$APP" >/dev/null
echo "Built $APP ($(du -sh "$APP" | cut -f1))"
