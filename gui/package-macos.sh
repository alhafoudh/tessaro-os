#!/bin/sh
# Package an already-built host binary beside itself; stdout is the app path.
set -eu
[ "$(uname -s)" = Darwin ] || { echo "macOS is required" >&2; exit 1; }
binary=${1:?usage: package-macos.sh /path/to/tessaro-gui}
[ -x "$binary" ] || { echo "Missing executable: $binary" >&2; exit 1; }
gui_dir=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
output_dir=$(CDPATH= cd -- "$(dirname "$binary")" && pwd)
app="$output_dir/Tessaro.app"
version=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$gui_dir/Cargo.toml")
[ -n "$version" ] || { echo "Missing GUI workspace version" >&2; exit 1; }
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/tessaro-gui"
cp "$gui_dir/tessaro-gui/icons/Tessaro.icns" "$app/Contents/Resources/Tessaro.icns"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleExecutable</key><string>tessaro-gui</string>
    <key>CFBundleIdentifier</key><string>sk.tessaro.gui</string>
    <key>CFBundleName</key><string>Tessaro</string>
    <key>CFBundleDisplayName</key><string>Tessaro</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleIconFile</key><string>Tessaro.icns</string>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Tessaro discovers and connects to kiosks on your local network.</string>
</dict>
</plist>
EOF
# A local signature covers the executable, metadata and icon together.
codesign --force --sign - "$app" >&2
printf '%s\n' "$app"
